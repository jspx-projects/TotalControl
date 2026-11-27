// SPDX-License-Identifier: GPL-2.0-only
/*
 * RME Fireface USB - MIDI over interface 2.
 *
 * The MIDI function is one interface with one bulk pair: OUT 0x07 and IN
 * 0x86.  Transfers are series of 4-byte packets:
 *
 *	[port:4 | code:4] [b1] [b2] [b3]
 *
 * The high nibble selects one of the logical MIDI ports carried on the
 * pair.  Windows exposes the UC ports as "UC Midi Port 1" (nibble 0) and
 * "UC Midi Port 2" (nibble 1).  The low nibble is a USB-MIDI 1.0 code
 * index number in both directions, and the packet carries one to three
 * MIDI bytes.
 *
 * The UC declares interface 2 as vendor-specific.  It still accepts and
 * sends USB-MIDI 1.0 packets: RME's Windows driver sends length codes in
 * the low nibble, the device accepts class codes as well, and the
 * device-to-host direction always uses class codes.  This driver uses the
 * class code for every unit, including the UC.
 *
 * The unit parses each bulk transfer as a fresh MIDI stream.  A message
 * that spans two transfers loses the continuation, so a caller must write
 * a message in one piece that fits the output buffer.
 *
 * The vendor driver owns the device, so this file claims the streaming
 * interface and republishes each logical port as one ALSA rawmidi device.
 * The input URBs run only while at least one input substream is open.
 */

#include <linux/delay.h>
#include <linux/slab.h>
#include <linux/usb.h>
#include <sound/core.h>
#include <sound/rawmidi.h>

#include "ff.h"

#define FFUC_MIDI_PACKET	4
#define FFUC_MIDI_IN_BYTES	512
/* One bulk packet is at most 512 bytes, so at most 128 event packets. */
#define FFUC_MIDI_OUT_BYTES	128
#define FFUC_MIDI_OUT_PACKETS	128
#define FFUC_MIDI_OUT_INTERVAL	1	/* ms between output polls */
#define FFUC_MIDI_OUT_TIMEOUT	1000	/* ms per bulk transfer */

/* USB-MIDI 1.0 code index number -> payload bytes. */
static const u8 ff_midi_cin_len[16] = {
	0, 0, 2, 3, 3, 1, 2, 3,
	3, 3, 3, 3, 2, 2, 3, 1,
};

static unsigned int ff_midi_code_len(u8 code)
{
	return ff_midi_cin_len[code & 0x0f];
}

static void ff_midi_in_complete(struct urb *urb)
{
	struct ff *chip = urb->context;
	unsigned int i;

	if (urb->status) {
		if (urb->status != -ENOENT && urb->status != -ECONNRESET &&
		    urb->status != -ESHUTDOWN && urb->status != -EPROTO)
			dev_dbg_ratelimited(&chip->dev->dev,
					    "MIDI IN status %d\n", urb->status);
		/* A bulk IN URB that completes with an error stops the
		 * stream unless it is queued again. */
		if (READ_ONCE(chip->midi_in_running))
			usb_submit_urb(urb, GFP_ATOMIC);
		return;
	}
	if (urb->actual_length)
		dev_dbg(&chip->dev->dev, "MIDI IN %u bytes %*ph\n",
			urb->actual_length, (int)urb->actual_length,
			urb->transfer_buffer);

	for (i = 0; i + FFUC_MIDI_PACKET <= urb->actual_length;
	     i += FFUC_MIDI_PACKET) {
		const u8 *pkt = urb->transfer_buffer + i;
		struct ff_midi_port *port;
		struct snd_rawmidi_substream *sub;
		unsigned int len;

		if (pkt[0] >> 4 >= chip->n_midi_ports)
			continue;
		len = ff_midi_code_len(pkt[0]);
		if (!len)
			continue;
		port = &chip->midi_port[pkt[0] >> 4];
		sub = READ_ONCE(port->in_sub);
		if (sub)
			snd_rawmidi_receive(sub, pkt + 1, len);
	}

	if (READ_ONCE(chip->midi_in_running))
		usb_submit_urb(urb, GFP_ATOMIC);
}

static void ff_midi_in_start(struct ff *chip)
{
	unsigned int i;
	int ret;

	if (!chip->midi_intf || READ_ONCE(chip->disconnected) ||
	    READ_ONCE(chip->midi_in_running))
		return;

	WRITE_ONCE(chip->midi_in_running, true);
	for (i = 0; i < FFUC_MIDI_IN_URBS; i++) {
		struct urb *urb = chip->midi_in_urb[i];

		usb_fill_bulk_urb(urb, chip->dev,
				  usb_rcvbulkpipe(chip->dev,
						  chip->desc->midi_in_ep),
				  chip->midi_in_buf[i], FFUC_MIDI_IN_BYTES,
				  ff_midi_in_complete, chip);
		ret = usb_submit_urb(urb, GFP_ATOMIC);
		if (ret < 0) {
			dev_dbg(&chip->dev->dev,
				"MIDI IN submit failed: %d\n", ret);
			WRITE_ONCE(chip->midi_in_running, false);
			break;
		}
	}
}

static void ff_midi_in_stop(struct ff *chip)
{
	unsigned int i;

	WRITE_ONCE(chip->midi_in_running, false);
	for (i = 0; i < FFUC_MIDI_IN_URBS; i++)
		usb_kill_urb(chip->midi_in_urb[i]);
}

/* -- host to unit ---------------------------------------------------- */

static unsigned int ff_midi_emit(unsigned int port,
				   u8 *out, unsigned int npkt, u8 code,
				   const u8 *data, unsigned int len)
{
	u8 *pkt;

	if (npkt >= FFUC_MIDI_OUT_PACKETS)
		return npkt;

	pkt = out + npkt * FFUC_MIDI_PACKET;
	pkt[0] = (port << 4) | code;
	pkt[1] = data[0];
	pkt[2] = len > 1 ? data[1] : 0;
	pkt[3] = len > 2 ? data[2] : 0;
	return npkt + 1;
}

/* Convert a raw MIDI byte stream into 4-byte packets.  Partial messages
 * are kept in the port state across calls. */
static unsigned int ff_midi_pack(unsigned int port,
				   struct ff_midi_tx *tx, const u8 *in,
				   unsigned int len, u8 *out)
{
	unsigned int i, npkt = 0;

	for (i = 0; i < len; i++) {
		u8 b = in[i];
		unsigned int need, cin;

		if (b >= 0xf8) {	/* realtime, may interleave */
			npkt = ff_midi_emit(port, out, npkt, 0xf, &b, 1);
			continue;
		}
		if (b == 0xf0) {
			tx->in_sysex = true;
			tx->sysex_len = 0;
			tx->msg_len = 0;
			tx->sysex[tx->sysex_len++] = b;
			continue;
		}
		if (b == 0xf7) {
			if (tx->in_sysex) {
				/* Include the terminator in the end packet:
				 * one byte gives code 5, two gives 6. */
				tx->sysex[tx->sysex_len++] = b;
				npkt = ff_midi_emit(port, out, npkt,
						      4 + tx->sysex_len,
						      tx->sysex, tx->sysex_len);
				tx->sysex_len = 0;
			} else {
				npkt = ff_midi_emit(port, out, npkt,
						      0xf, &b, 1);
			}
			tx->in_sysex = false;
			continue;
		}
		if (tx->in_sysex) {
			tx->sysex[tx->sysex_len++] = b;
			if (tx->sysex_len == 3) {
				npkt = ff_midi_emit(port, out, npkt, 4,
						      tx->sysex, 3);
				tx->sysex_len = 0;
			}
			continue;
		}
		if (b & 0x80) {
			if (b == 0xf6) {	/* tune request: one byte */
				npkt = ff_midi_emit(port, out, npkt,
						      0xf, &b, 1);
				continue;
			}
			/* F4 and F5 are undefined; drop them and any data
			 * that would otherwise complete a message. */
			if (b == 0xf4 || b == 0xf5) {
				tx->status = 0;
				tx->msg_len = 0;
				continue;
			}
			tx->status = b;
			tx->msg_len = 0;
			continue;
		}
		if (!tx->status)
			continue;	/* stray data byte */
		tx->msg[tx->msg_len++] = b;
		if ((tx->status & 0xf0) == 0xf0) {
			/* system common: F1/F3 take one data byte, F2 two */
			need = tx->status == 0xf2 ? 2 : 1;
			cin = tx->status == 0xf2 ? 3 : 2;
		} else {
			need = ((tx->status & 0xf0) == 0xc0 ||
				(tx->status & 0xf0) == 0xd0) ? 1 : 2;
			cin = tx->status >> 4;
		}
		if (tx->msg_len >= need) {
			u8 msg[3] = { tx->status, tx->msg[0], tx->msg[1] };

			npkt = ff_midi_emit(port, out, npkt, cin, msg,
					      need + 1);
			tx->msg_len = 0;
		}
	}
	return npkt;
}

static void ff_midi_out_work(struct work_struct *work)
{
	struct ff_midi_port *port = container_of(work,
						   struct ff_midi_port,
						   out_work.work);
	struct ff *chip = port->chip;
	struct snd_rawmidi_substream *substream;
	u8 in[FFUC_MIDI_OUT_BYTES];
	u8 *out = port->out_buf;

	if (READ_ONCE(chip->disconnected) || !READ_ONCE(port->out_running))
		return;
	substream = READ_ONCE(port->out_sub);
	if (!substream || !out)
		return;

	for (;;) {
		unsigned int npkt;
		int n, sent = 0, ret;

		n = snd_rawmidi_transmit_peek(substream, in, sizeof(in));
		if (n <= 0)
			break;
		npkt = ff_midi_pack(port->index, &port->tx, in, n, out);
		if (npkt) {
			ret = usb_bulk_msg(chip->dev,
					   usb_sndbulkpipe(chip->dev,
							   chip->desc->midi_out_ep),
					   out, npkt * FFUC_MIDI_PACKET, &sent,
					   FFUC_MIDI_OUT_TIMEOUT);
			if (ret < 0) {
				dev_warn_ratelimited(&chip->dev->dev,
						     "MIDI OUT %u failed: %d\n",
						     port->index + 1, ret);
				/* A stalled bulk endpoint stays halted until
				 * the host clears it; every later transfer
				 * would fail the same way. */
				if (ret == -EPIPE)
					usb_clear_halt(chip->dev,
						       usb_sndbulkpipe(chip->dev,
								       chip->desc->midi_out_ep));
				/* Drop the chunk; a rawmidi writer must not
				 * spin on data the unit refused. */
				snd_rawmidi_transmit_ack(substream, n);
				break;
			}
		}
		snd_rawmidi_transmit_ack(substream, n);
	}

	if (READ_ONCE(port->out_running))
		schedule_delayed_work(&port->out_work,
				      msecs_to_jiffies(FFUC_MIDI_OUT_INTERVAL));
}

/* -- ALSA rawmidi ---------------------------------------------------- */

static int ff_midi_input_open(struct snd_rawmidi_substream *substream)
{
	struct ff_midi_port *port = substream->rmidi->private_data;

	WRITE_ONCE(port->in_sub, substream);
	if (atomic_inc_return(&port->chip->midi_in_users) == 1)
		ff_midi_in_start(port->chip);
	return 0;
}

static int ff_midi_input_close(struct snd_rawmidi_substream *substream)
{
	struct ff_midi_port *port = substream->rmidi->private_data;
	struct ff *chip = port->chip;

	WRITE_ONCE(port->in_sub, NULL);
	if (atomic_dec_and_test(&chip->midi_in_users))
		ff_midi_in_stop(chip);
	return 0;
}

static void ff_midi_input_trigger(struct snd_rawmidi_substream *substream,
				    int up)
{
	/* The URBs follow open/close; several clients may share the
	 * substream and a second trigger must not resubmit live URBs. */
}

static int ff_midi_output_open(struct snd_rawmidi_substream *substream)
{
	struct ff_midi_port *port = substream->rmidi->private_data;

	memset(&port->tx, 0, sizeof(port->tx));
	WRITE_ONCE(port->out_sub, substream);
	return 0;
}

static int ff_midi_output_close(struct snd_rawmidi_substream *substream)
{
	struct ff_midi_port *port = substream->rmidi->private_data;

	WRITE_ONCE(port->out_running, false);
	cancel_delayed_work_sync(&port->out_work);
	WRITE_ONCE(port->out_sub, NULL);
	return 0;
}

static void ff_midi_output_trigger(struct snd_rawmidi_substream *substream,
				     int up)
{
	struct ff_midi_port *port = substream->rmidi->private_data;

	if (up) {
		WRITE_ONCE(port->out_running, true);
		schedule_delayed_work(&port->out_work, 0);
	} else {
		WRITE_ONCE(port->out_running, false);
		cancel_delayed_work_sync(&port->out_work);
	}
}

static void ff_midi_output_drain(struct snd_rawmidi_substream *substream)
{
	struct ff_midi_port *port = substream->rmidi->private_data;

	if (!READ_ONCE(port->out_running))
		return;
	flush_delayed_work(&port->out_work);
}

static const struct snd_rawmidi_ops ff_midi_input_ops = {
	.open = ff_midi_input_open,
	.close = ff_midi_input_close,
	.trigger = ff_midi_input_trigger,
};

static const struct snd_rawmidi_ops ff_midi_output_ops = {
	.open = ff_midi_output_open,
	.close = ff_midi_output_close,
	.trigger = ff_midi_output_trigger,
	.drain = ff_midi_output_drain,
};

/* -- setup / teardown ------------------------------------------------ */

void ff_midi_init(struct ff *chip)
{
	unsigned int i;

	atomic_set(&chip->midi_in_users, 0);
	for (i = 0; i < FFUC_MIDI_PORTS; i++) {
		struct ff_midi_port *port = &chip->midi_port[i];

		port->chip = chip;
		port->index = i;
		INIT_DELAYED_WORK(&port->out_work, ff_midi_out_work);
	}
}

static bool ff_midi_interface(const struct usb_interface *intf,
				const struct ff_device_desc *desc)
{
	const struct usb_host_interface *alt = intf->cur_altsetting;
	const struct usb_endpoint_descriptor *ep;
	bool in = false, out = false;
	int i;

	for (i = 0; i < alt->desc.bNumEndpoints; i++) {
		ep = &alt->endpoint[i].desc;
		if (usb_endpoint_is_bulk_in(ep) &&
		    ep->bEndpointAddress == desc->midi_in_ep)
			in = true;
		else if (usb_endpoint_is_bulk_out(ep) &&
			 ep->bEndpointAddress == desc->midi_out_ep)
			out = true;
	}
	return in && out;
}

static void ff_midi_release_ports(struct ff *chip)
{
	unsigned int i;

	for (i = 0; i < chip->n_midi_ports; i++) {
		struct ff_midi_port *port = &chip->midi_port[i];

		if (port->rmidi) {
			snd_rawmidi_free(port->rmidi);
			port->rmidi = NULL;
		}
		kfree(port->out_buf);
		port->out_buf = NULL;
	}
	chip->n_midi_ports = 0;
}

/* A MIDI-streaming interface declares its logical ports in the
 * class-specific endpoint descriptor: one embedded jack per port.  A
 * vendor-specific interface, as on the UC, has no such descriptor and
 * takes the port count from the device profile. */
static unsigned int ff_midi_class_ports(const struct usb_host_endpoint *ep)
{
	const u8 *p = ep->extra;
	unsigned int left = ep->extralen;

	while (left >= 3) {
		unsigned int blen = p[0];

		if (blen < 3 || blen > left)
			break;
		if (p[1] == USB_DT_CS_ENDPOINT && p[2] == 1 && blen >= 4)
			return p[3];
		p += blen;
		left -= blen;
	}
	return 0;
}

static unsigned int ff_midi_port_count(const struct usb_interface *intf,
					 const struct ff_device_desc *desc)
{
	const struct usb_host_interface *alt = intf->cur_altsetting;
	unsigned int i, ports = 0;

	for (i = 0; i < alt->desc.bNumEndpoints; i++) {
		unsigned int n = ff_midi_class_ports(&alt->endpoint[i]);

		if (n > ports)
			ports = n;
	}
	return ports ?: (desc->midi_ports ?: 1);
}

int ff_midi_create(struct ff *chip)
{
	struct usb_interface *intf;
	unsigned int i;
	int ret;

	if (!chip->desc->midi_out_ep || !chip->desc->midi_in_ep)
		return 0;
	intf = usb_ifnum_to_if(chip->dev, chip->desc->midi_iface);
	if (!intf || !ff_midi_interface(intf, chip->desc))
		return -ENODEV;

	if (ff_claim_interface(intf) < 0)
		return -EBUSY;
	chip->midi_intf = intf;
	chip->n_midi_ports = ff_midi_port_count(intf, chip->desc);
	if (chip->n_midi_ports > FFUC_MIDI_PORTS) {
		dev_info(&chip->dev->dev,
			 "MIDI: %u ports on the endpoint pair, exposing %u\n",
			 chip->n_midi_ports, FFUC_MIDI_PORTS);
		chip->n_midi_ports = FFUC_MIDI_PORTS;
	}

	for (i = 0; i < FFUC_MIDI_IN_URBS; i++) {
		chip->midi_in_urb[i] = usb_alloc_urb(0, GFP_KERNEL);
		chip->midi_in_buf[i] = kmalloc(FFUC_MIDI_IN_BYTES, GFP_KERNEL);
		if (!chip->midi_in_urb[i] || !chip->midi_in_buf[i]) {
			ret = -ENOMEM;
			goto err;
		}
	}

	for (i = 0; i < chip->n_midi_ports; i++) {
		struct ff_midi_port *port = &chip->midi_port[i];
		struct snd_rawmidi *rmidi;
		char id[32];

		port->out_buf = kmalloc(FFUC_MIDI_OUT_PACKETS *
					FFUC_MIDI_PACKET, GFP_KERNEL);
		if (!port->out_buf) {
			ret = -ENOMEM;
			goto err;
		}
		strscpy(id, chip->desc->shortname, sizeof(id));
		ret = snd_rawmidi_new(chip->card, id, i, 1, 1, &rmidi);
		if (ret < 0)
			goto err;

		port->rmidi = rmidi;
		rmidi->private_data = port;
		rmidi->info_flags = SNDRV_RAWMIDI_INFO_OUTPUT |
				    SNDRV_RAWMIDI_INFO_INPUT |
				    SNDRV_RAWMIDI_INFO_DUPLEX;
		snprintf(rmidi->name, sizeof(rmidi->name), "%s MIDI %u",
			 chip->desc->name, i + 1);
		snd_rawmidi_set_ops(rmidi, SNDRV_RAWMIDI_STREAM_INPUT,
				    &ff_midi_input_ops);
		snd_rawmidi_set_ops(rmidi, SNDRV_RAWMIDI_STREAM_OUTPUT,
				    &ff_midi_output_ops);
	}
	return 0;

err:
	ff_midi_release_ports(chip);
	for (i = 0; i < FFUC_MIDI_IN_URBS; i++) {
		usb_free_urb(chip->midi_in_urb[i]);
		chip->midi_in_urb[i] = NULL;
		kfree(chip->midi_in_buf[i]);
		chip->midi_in_buf[i] = NULL;
	}
	ff_release_interface(intf);
	chip->midi_intf = NULL;
	return ret;
}

void ff_midi_cleanup(struct ff *chip)
{
	unsigned int i;

	WRITE_ONCE(chip->midi_in_running, false);
	for (i = 0; i < FFUC_MIDI_PORTS; i++) {
		struct ff_midi_port *port = &chip->midi_port[i];

		WRITE_ONCE(port->out_running, false);
		cancel_delayed_work_sync(&port->out_work);
		WRITE_ONCE(port->in_sub, NULL);
		WRITE_ONCE(port->out_sub, NULL);
		kfree(port->out_buf);
		port->out_buf = NULL;
	}
	for (i = 0; i < FFUC_MIDI_IN_URBS; i++) {
		if (chip->midi_in_urb[i]) {
			usb_kill_urb(chip->midi_in_urb[i]);
			usb_free_urb(chip->midi_in_urb[i]);
			chip->midi_in_urb[i] = NULL;
		}
		kfree(chip->midi_in_buf[i]);
		chip->midi_in_buf[i] = NULL;
	}
	if (chip->midi_intf) {
		ff_release_interface(chip->midi_intf);
		chip->midi_intf = NULL;
	}
}

void ff_midi_suspend(struct ff *chip)
{
	unsigned int i;

	for (i = 0; i < FFUC_MIDI_PORTS; i++) {
		struct ff_midi_port *port = &chip->midi_port[i];

		WRITE_ONCE(port->out_running, false);
		cancel_delayed_work_sync(&port->out_work);
	}
	ff_midi_in_stop(chip);
}

void ff_midi_resume(struct ff *chip)
{
	unsigned int i;

	if (!chip->midi_intf || READ_ONCE(chip->disconnected))
		return;
	if (atomic_read(&chip->midi_in_users))
		ff_midi_in_start(chip);
	for (i = 0; i < chip->n_midi_ports; i++) {
		struct ff_midi_port *port = &chip->midi_port[i];

		if (!READ_ONCE(port->out_sub))
			continue;
		WRITE_ONCE(port->out_running, true);
		schedule_delayed_work(&port->out_work, 0);
	}
}
