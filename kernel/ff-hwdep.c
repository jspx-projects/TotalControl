// SPDX-License-Identifier: GPL-2.0-only
/*
 * RME Fireface USB - snd-hwdep control transport.
 *
 * One hwdep node per card exposes:
 *
 *   ioctl(INFO)          identity, capabilities, endpoint numbers
 *   ioctl(VENDOR)        one EP0 vendor request (read or write, with data)
 *   ioctl(WRITE_WORDS)   one or more IF1 register words (parity added here)
 *   ioctl(COUNTERS)      transport health counters
 *   ioctl(SET_RATE)      set the device clock rate while idle (no PCM owner)
 *   read()/poll()        meter, IF1 state and status events
 *
 * The IF1 meter (0x85) and state (0x8d) endpoints are read continuously by
 * this driver; userspace receives raw packets/words and owns the parsing,
 * because meter layouts and register semantics are device-model policy.
 */
#include <linux/kfifo.h>
#include <linux/module.h>
#include <linux/poll.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/uaccess.h>
#include <linux/usb.h>
#include <sound/core.h>
#include <sound/hwdep.h>

#include "ff.h"
#include "ff-meter-pair.h"

/* -- event queue ----------------------------------------------------- */

static bool ff_event_ready(struct ff *chip)
{
	struct ff_hwdep_event_hdr h;
	unsigned long flags;
	bool ready = false;

	spin_lock_irqsave(&chip->event_lock, flags);
	if (kfifo_out_peek(&chip->event_fifo, &h, sizeof(h)) == sizeof(h) &&
	    kfifo_len(&chip->event_fifo) >= sizeof(h) + h.size)
		ready = true;
	spin_unlock_irqrestore(&chip->event_lock, flags);
	return ready;
}

void ff_event_put(struct ff *chip, u32 type, const void *data, u32 size,
		    u32 flags)
{
	struct ff_hwdep_event_hdr h;
	unsigned long irqflags;

	if (!chip->hwdep || READ_ONCE(chip->disconnected))
		return;
	if (size > FF_EVENT_MAX)
		size = FF_EVENT_MAX;

	h.type = type;
	h.size = size;
	h.seq = atomic_inc_return(&chip->event_seq);
	h.flags = flags;

	spin_lock_irqsave(&chip->event_lock, irqflags);
	if (kfifo_avail(&chip->event_fifo) < sizeof(h) + size) {
		atomic_inc(&chip->events_dropped);
		spin_unlock_irqrestore(&chip->event_lock, irqflags);
		return;
	}
	kfifo_in(&chip->event_fifo, &h, sizeof(h));
	if (size)
		kfifo_in(&chip->event_fifo, data, size);
	spin_unlock_irqrestore(&chip->event_lock, irqflags);
	wake_up_interruptible(&chip->event_wait);
}

static ssize_t ff_hwdep_read(struct snd_hwdep *hwdep, char __user *buf,
			       long count, loff_t *offset)
{
	struct ff *chip = hwdep->private_data;
	struct ff_hwdep_event_hdr h;
	u8 *tmp;
	unsigned int len, got;
	unsigned long flags;
	bool was_paused;
	int ret;

	if (count < (long)sizeof(h))
		return -EINVAL;
	if (!ff_event_ready(chip)) {
		ret = wait_event_interruptible(chip->event_wait,
					       ff_event_ready(chip) ||
					       READ_ONCE(chip->disconnected));
		if (ret)
			return ret;
	}
	if (READ_ONCE(chip->disconnected))
		return -ENODEV;

	tmp = kmalloc(FF_EVENT_BUF_SIZE, GFP_KERNEL);
	if (!tmp)
		return -ENOMEM;

	spin_lock_irqsave(&chip->event_lock, flags);
	if (kfifo_out_peek(&chip->event_fifo, &h, sizeof(h)) != sizeof(h) ||
	    kfifo_len(&chip->event_fifo) < sizeof(h) + h.size) {
		spin_unlock_irqrestore(&chip->event_lock, flags);
		kfree(tmp);
		return -EAGAIN;
	}
	len = sizeof(h) + h.size;
	if (count < (long)len) {
		spin_unlock_irqrestore(&chip->event_lock, flags);
		kfree(tmp);
		return -EINVAL;
	}
	was_paused = READ_ONCE(chip->meter_paused);
	got = kfifo_out(&chip->event_fifo, tmp, len);
	if (was_paused &&
	    kfifo_len(&chip->event_fifo) <
		    FF_METER_LOW_WATER *
			    (chip->desc ? chip->desc->meter_len * 2 + 16 : 16))
		WRITE_ONCE(chip->meter_paused, false);
	spin_unlock_irqrestore(&chip->event_lock, flags);
	if (got != len) {
		kfree(tmp);
		return -EIO;
	}
	if (was_paused && chip->state_active &&
	    !READ_ONCE(chip->meter_paused))
		schedule_delayed_work(&chip->meter_work, 0);

	if (copy_to_user(buf, tmp, len))
		ret = -EFAULT;
	else
		ret = len;
	kfree(tmp);
	return ret;
}

static __poll_t ff_hwdep_poll(struct snd_hwdep *hwdep, struct file *file,
				poll_table *wait)
{
	struct ff *chip = hwdep->private_data;
	__poll_t mask = 0;

	poll_wait(file, &chip->event_wait, wait);
	if (READ_ONCE(chip->disconnected))
		return EPOLLERR | EPOLLHUP;
	if (ff_event_ready(chip))
		mask |= EPOLLIN | EPOLLRDNORM;
	return mask;
}

static int ff_hwdep_open(struct snd_hwdep *hwdep, struct file *file)
{
	struct ff *chip = hwdep->private_data;

	if (atomic_inc_return(&chip->hwdep_users) == 1)
		ff_transport_start(chip);
	return 0;
}

static int ff_hwdep_release(struct snd_hwdep *hwdep, struct file *file)
{
	struct ff *chip = hwdep->private_data;

	if (atomic_dec_and_test(&chip->hwdep_users)) {
		unsigned long flags;

		ff_transport_stop(chip);
		if (chip->caps & FF_TCAP_METER_PAIR) {
			/* Unread events are stale for the next reader. */
			spin_lock_irqsave(&chip->event_lock, flags);
			kfifo_reset(&chip->event_fifo);
			spin_unlock_irqrestore(&chip->event_lock, flags);
			WRITE_ONCE(chip->meter_paused, false);
		}
	}
	return 0;
}

/* -- ioctls ---------------------------------------------------------- */

static int ff_hwdep_info(struct ff *chip,
			   struct ff_hwdep_info __user *arg)
{
	struct ff_hwdep_info info;
	u32 caps = FF_CAP_VENDOR | FF_CAP_PCM |
		   FF_CAP_SNAPSHOT | FF_CAP_DIAGNOSTICS;

	if (chip->desc->state_out_ep)
		caps |= FF_CAP_WORDS;
	if (chip->desc->meter_in_ep && chip->desc->meter_len)
		caps |= FF_CAP_METERS;
	if (chip->desc->status_iface != FF_IFACE_NONE)
		caps |= FF_CAP_STATUS;

	memset(&info, 0, sizeof(info));
	info.abi = FF_ABI_VERSION;
	info.caps = caps;
	info.transport = chip->iso ? FF_XPORT_ISO : FF_XPORT_INTERRUPT;
	info.channels = chip->desc->max_channels;
	info.inputs = chip->desc->max_channels;
	info.outputs = ff_desc_playback(chip->desc);
	info.playback = ff_desc_playback(chip->desc);
	info.matrix_sources = chip->desc->max_channels +
			      ff_desc_playback(chip->desc);
	info.meter_bytes = chip->desc->meter_len;
	info.state_ep = chip->desc->state_in_ep;
	info.meter_ep = chip->desc->meter_in_ep;
	info.status_ep = chip->desc->status_ep;
	info.rate = chip->rate;
	info.firmware = chip->firmware;
	info.card = chip->card->number;
	info.pcm_device = 0;
	info.vendor = chip->desc->vendor;
	info.product = chip->desc->product;
	info.bcd_device = le16_to_cpu(chip->dev->descriptor.bcdDevice);
	strscpy(info.serial, chip->serial, sizeof(info.serial));

	if (copy_to_user(arg, &info, sizeof(info)))
		return -EFAULT;
	return 0;
}

static int ff_hwdep_vendor(struct ff *chip,
			     struct ff_hwdep_vendor __user *arg)
{
	struct ff_hwdep_vendor v;
	void *data = NULL;
	int ret;

	if (copy_from_user(&v, arg, sizeof(v)))
		return -EFAULT;
	if (v.direction > 1)
		return -EINVAL;
	if (v.length > FF_HWDEP_MAX_DATA)
		return -EINVAL;

	if (v.length) {
		data = kmalloc(v.length, GFP_KERNEL);
		if (!data)
			return -ENOMEM;
		if (!v.direction)
			memcpy(data, v.data, v.length);
	}

	mutex_lock(&chip->ctl_lock);
	if (v.direction)
		ret = ff_vendor_recv(chip, v.request, v.value, v.index,
				       data, v.length);
	else {
		/* The pitch envelope: the ISO transport carries at most
		 * max_frames frames per microframe packet. A DDS divisor that
		 * programs a clock above 8000 * max_frames drives the device
		 * into a folded capture cadence and transport errors, so the
		 * active family's word clamps to the ceiling. The other
		 * families store their pitch for later rates and pass
		 * untouched; the rate sequence writes nominal words, which
		 * are always in envelope. */
		if (v.request == FF_REQ_DDS && chip->iso && chip->rate &&
		    chip->mode &&
		    (v.index & 0x03) == ff_rate_family(chip->rate)) {
			u32 val = v.value, idx = v.index;

			if (ff_dds_clamp(val, idx, chip->rate,
					   chip->mode->max_frames, &val, &idx)) {
				atomic_inc(&chip->pitch_clamps);
				v.value = (u16)val;
				v.index = (u16)idx;
			}
		}
		ret = ff_vendor_send(chip, v.request, v.value, v.index,
				       data, v.length);
	}
	mutex_unlock(&chip->ctl_lock);

	if (ret >= 0 && v.direction && v.length)
		memcpy(v.data, data, v.length);
	kfree(data);
	if (ret < 0)
		return ret;
	if (copy_to_user(arg, &v, sizeof(v)))
		return -EFAULT;
	return 0;
}

static int ff_hwdep_words(struct ff *chip,
			    struct ff_hwdep_words __user *arg)
{
	struct ff_hwdep_words w;

	if (copy_from_user(&w, arg, sizeof(w)))
		return -EFAULT;
	if (w.count > FF_HWDEP_MAX_WORDS)
		return -EINVAL;
	/*
	 * ff_reg_send() owns chip->ctl_lock.  Do not take it here:
	 * the mutex is not recursive, and the UCX IF1 path would deadlock
	 * in the ioctl before the first word reaches the device.
	 */
	return ff_reg_send(chip, w.words, w.count);
}

/* Live snapshot: queue state for both directions and the device clock in
 * one sample.  The caller sets `size`; the driver fills up to that size. */
static int ff_hwdep_snapshot(struct ff *chip,
			     struct ff_hwdep_snapshot __user *arg)
{
	struct ff_hwdep_snapshot s;
	unsigned long flags;
	u32 size;
	int dir;

	if (copy_from_user(&size, &arg->size, sizeof(size)))
		return -EFAULT;
	if (size < sizeof(s.abi) + sizeof(s.size))
		return -EINVAL;
	if (size > sizeof(s))
		size = sizeof(s);

	memset(&s, 0, sizeof(s));
	s.abi = FF_ABI_VERSION;
	s.size = sizeof(s);
	s.run_seq = chip->run_seq;
	s.rate = chip->rate;
	spin_lock_irqsave(&chip->fb_lock, flags);
	s.timestamp_ns = chip->ts_ns;
	s.timestamp_pos = chip->ts_pos;
	s.timestamp_phase = chip->ts_phase;
	s.rate_meas = chip->ts_rate_meas;
	s.anchor_ns = chip->ts_anchor_ns;
	s.frames = chip->ts_frames;
	s.wrap = ff_iso_wrap(chip);
	spin_unlock_irqrestore(&chip->fb_lock, flags);

	for (dir = 0; dir < 2; dir++) {
		struct ff_hwdep_stream *st = dir ? &s.in : &s.out;
		struct ff_stream *fs = &chip->streams[dir];
		struct snd_pcm_substream *subs = fs->substream;
		struct snd_pcm_runtime *rt;
		u64 boundary;

		if (!subs || !(rt = subs->runtime))
			continue;
		st->present = 1;
		st->state = rt->status->state;
		st->appl_ptr = READ_ONCE(rt->control->appl_ptr);
		st->hw_ptr = fs->hw_ptr;
		st->period = rt->period_size;
		st->buffer = rt->buffer_size;
		boundary = rt->boundary;
		if (dir == SNDRV_PCM_STREAM_PLAYBACK)
			st->fill = (st->appl_ptr + boundary - st->hw_ptr) %
				   boundary;
		else
			st->fill = (st->hw_ptr + boundary - st->appl_ptr) %
				   boundary;
		if (st->fill > st->buffer)
			st->fill = st->buffer;
		st->inflight = atomic_read(&fs->inflight);
		st->delay = st->fill + st->inflight;
		st->urb_frames = chip->urb_frames;
		st->nurbs = chip->nurbs;
		st->iso_packets = chip->iso_packets;
		st->device_pos = dir == SNDRV_PCM_STREAM_CAPTURE ?
				 chip->in_end : fs->pos[0];
		st->xruns = dir == SNDRV_PCM_STREAM_CAPTURE ?
			    atomic_read(&chip->in_overruns) :
			    atomic_read(&chip->out_underruns);
	}
	if (copy_to_user(arg, &s, size))
		return -EFAULT;
	return 0;
}

static int ff_hwdep_diagnostics(struct ff *chip,
				  struct ff_hwdep_diagnostics __user *arg)
{
	struct ff_hwdep_diagnostics c;

	memset(&c, 0, sizeof(c));
	c.out_urbs = atomic_read(&chip->out_urbs);
	c.out_gaps = atomic_read(&chip->out_gaps);
	c.out_shorts = atomic_read(&chip->out_shorts);
	c.out_status_errors = atomic_read(&chip->out_status_errors);
	c.urb_errors = atomic_read(&chip->urb_errors);
	c.recoveries = chip->recoveries;
	c.events_dropped = atomic_read(&chip->events_dropped);
	c.meter_urbs = atomic_read(&chip->meter_urbs);
	c.state_urbs = atomic_read(&chip->state_urbs);
	c.status_urbs = 0;
	c.in_urbs = atomic_read(&chip->in_urbs);
	c.in_gaps = atomic_read(&chip->in_gaps);
	c.in_shorts = atomic_read(&chip->in_shorts);
	c.in_status_errors = atomic_read(&chip->in_status_errors);
	c.out_underruns = atomic_read(&chip->out_underruns);
	c.in_overruns = atomic_read(&chip->in_overruns);
	c.out_capacity_err = atomic_read(&chip->out_capacity_err);
	c.out_pkt_len_err = atomic_read(&chip->out_pkt_len_err);
	c.out_pkt_status = atomic_read(&chip->out_pkt_status);
	c.in_status_err = atomic_read(&chip->in_status_err);
	c.in_short = atomic_read(&chip->in_short);
	c.in_misaligned = atomic_read(&chip->in_misaligned);
	c.in_bad_length = atomic_read(&chip->in_bad_length);
	c.out_bad_length = atomic_read(&chip->out_bad_length);
	c.in_pos_gap = atomic_read(&chip->in_pos_gap);
	c.in_frames_mismatch = atomic_read(&chip->in_frames_mismatch);
	c.rate_mismatch = atomic_read(&chip->rate_mismatch);
	c.out_fb_ahead = atomic_read(&chip->out_fb_ahead);
	c.out_fb_stale = atomic_read(&chip->out_fb_stale);
	c.hw_ptr_mismatch = atomic_read(&chip->hw_ptr_mismatch);
	c.delta_zero = atomic_read(&chip->delta_zero);
	c.run_seq = chip->run_seq;
	c.ring_min_filled = chip->ring_min_filled;
	c.ring_missed = atomic_read(&chip->ring_missed);
	c.ring_starved = atomic_read(&chip->ring_starved);
	c.ring_max_stall_ns = chip->ring_max_stall_ns;
	c.gate_deferred = atomic_read(&chip->signal_deferred);
	c.gate_overdue = atomic_read(&chip->signal_overdue);
	c.gate_max_pending = READ_ONCE(chip->max_pending);
	c.out_lag_min = chip->out_lag_min;
	c.out_lag_max = chip->out_lag_max;
	c.out_prefills = atomic_read(&chip->out_prefills);
	c.out_prefill_timeouts = atomic_read(&chip->out_prefill_timeouts);
	c.meter_paused = chip->meter_paused;
	c.meter_pairs = chip->meter_pairs;
	c.meter_realigns = chip->meter_realigns;
	c.in_total = chip->in_total;
	c.pitch_clamps = atomic_read(&chip->pitch_clamps);
	if (copy_to_user(arg, &c, sizeof(c)))
		return -EFAULT;
	return 0;
}

static int ff_hwdep_set_rate(struct ff *chip,
			       struct ff_hwdep_set_rate __user *arg)
{
	struct ff_hwdep_set_rate r;
	unsigned int confirmed = 0;
	int ret;

	if (copy_from_user(&r, arg, sizeof(r)))
		return -EFAULT;
	if (r.flags)
		return -EINVAL;
	ret = ff_set_rate_idle(chip, r.rate, &confirmed);
	if (ret < 0)
		return ret;
	r.confirmed = confirmed;
	if (copy_to_user(arg, &r, sizeof(r)))
		return -EFAULT;
	return 0;
}

static int ff_hwdep_ioctl(struct snd_hwdep *hwdep, struct file *file,
			    unsigned int cmd, unsigned long arg)
{
	struct ff *chip = hwdep->private_data;

	if (READ_ONCE(chip->disconnected))
		return -ENODEV;

	switch (cmd) {
	case FF_HWDEP_IOCTL_INFO:
		return ff_hwdep_info(chip, (void __user *)arg);
	case FF_HWDEP_IOCTL_VENDOR:
		return ff_hwdep_vendor(chip, (void __user *)arg);
	case FF_HWDEP_IOCTL_WRITE_WORDS:
		return ff_hwdep_words(chip, (void __user *)arg);
	case FF_HWDEP_IOCTL_DIAGNOSTICS:
		return ff_hwdep_diagnostics(chip, (void __user *)arg);
	case FF_HWDEP_IOCTL_SNAPSHOT:
		return ff_hwdep_snapshot(chip, (void __user *)arg);
	case FF_HWDEP_IOCTL_SET_RATE:
		return ff_hwdep_set_rate(chip, (void __user *)arg);
	case FF_HWDEP_IOCTL_ENVELOPE: {
		struct ff_hwdep_envelope env;

		ff_query_envelope(chip, &env);
		if (copy_to_user((void __user *)arg, &env, sizeof(env)))
			return -EFAULT;
		return 0;
	}
	default:
		return -ENOTTY;
	}
}

static const struct snd_hwdep_ops ff_hwdep_ops = {
	.open = ff_hwdep_open,
	.release = ff_hwdep_release,
	.read = ff_hwdep_read,
	.poll = ff_hwdep_poll,
	.ioctl = ff_hwdep_ioctl,
};

int ff_hwdep_create(struct ff *chip)
{
	struct snd_hwdep *hwdep;
	int ret;

	ret = snd_hwdep_new(chip->card, "Fireface", 0, &hwdep);
	if (ret < 0)
		return ret;
	hwdep->private_data = chip;
	hwdep->iface = SNDRV_HWDEP_IFACE_USB_STREAM;
	hwdep->ops = ff_hwdep_ops;
	strscpy(hwdep->name, "Fireface Control", sizeof(hwdep->name));
	chip->hwdep = hwdep;
	return 0;
}

/* -- IF1 meter/state transports -------------------------------------- */

static void ff_state_complete(struct urb *urb)
{
	struct ff *chip = urb->context;
	unsigned int i, words;

	atomic_inc(&chip->state_urbs);
	if (urb->status == 0 && urb->actual_length >= sizeof(u32)) {
		words = urb->actual_length / sizeof(u32);
		/* Split large reads into event-sized chunks. */
		for (i = 0; i < words; i += FF_EVENT_MAX / sizeof(u32)) {
			unsigned int n = min_t(unsigned int,
					       words - i,
					       FF_EVENT_MAX / sizeof(u32));

			ff_event_put(chip, FF_EV_STATE,
				       (const u8 *)urb->transfer_buffer +
				       i * sizeof(u32),
				       n * sizeof(u32), 0);
		}
	}

	switch (urb->status) {
	case -ENOENT:
	case -ECONNRESET:
	case -ESHUTDOWN:
	case -ENODEV:
		return;
	default:
		break;
	}
	if (chip->state_active && !READ_ONCE(chip->disconnected))
		schedule_delayed_work(&chip->state_work, msecs_to_jiffies(4));
}

/* Bytes currently queued in the event FIFO. */
static unsigned int ff_event_bytes(struct ff *chip)
{
	unsigned long flags;
	unsigned int len;

	spin_lock_irqsave(&chip->event_lock, flags);
	len = kfifo_len(&chip->event_fifo);
	spin_unlock_irqrestore(&chip->event_lock, flags);
	return len;
}

static void ff_meter_complete(struct urb *urb)
{
	struct ff *chip = urb->context;

	atomic_inc(&chip->meter_urbs);
	if (urb->status == 0 && chip->desc &&
	    (chip->caps & FF_TCAP_METER_PAIR) &&
	    urb->actual_length == chip->desc->meter_len &&
	    2 * chip->desc->meter_len <= FF_EVENT_MAX) {
		unsigned int len = chip->desc->meter_len;
		unsigned int flags = FF_EV_FLAG_METER_PAIR;

		switch (ff_meter_feed(chip->meter_pair, &chip->meter_pair_len,
					&chip->meter_phase_lost,
					urb->transfer_buffer, len)) {
		case FF_METER_PAIR_UNVERIFIED:
			flags |= FF_EV_FLAG_PHASE_LOST;
			fallthrough;
		case FF_METER_PAIR:
			ff_event_put(chip, FF_EV_METER, chip->meter_pair,
				       2 * len, flags);
			chip->meter_pairs++;
			/* Pause only at a pair boundary. */
			if (ff_event_bytes(chip) >
			    FF_METER_HIGH_WATER * (2 * len + 16))
				WRITE_ONCE(chip->meter_paused, true);
			break;
		case FF_METER_REALIGNED:
			chip->meter_realigns++;
			break;
		case FF_METER_HELD:
			break;
		}
	} else if (urb->status == 0 && urb->actual_length >= 4 &&
		   !(chip->caps & FF_TCAP_METER_PAIR)) {
		/* Self-describing packet devices (UCX, Babyface) pass each
		 * packet through unchanged. */
		ff_event_put(chip, FF_EV_METER, urb->transfer_buffer,
			       min_t(unsigned int, urb->actual_length,
				     FF_EVENT_MAX), 0);
	}

	switch (urb->status) {
	case -ENOENT:
	case -ECONNRESET:
	case -ESHUTDOWN:
	case -ENODEV:
		return;
	default:
		break;
	}
	if (chip->state_active && !READ_ONCE(chip->disconnected) &&
	    !chip->meter_paused)
		schedule_delayed_work(&chip->meter_work, msecs_to_jiffies(4));
}

static void ff_state_work_fn(struct work_struct *work)
{
	struct ff *chip = container_of(to_delayed_work(work), struct ff,
					 state_work);
	int ret;

	if (!chip->state_active || READ_ONCE(chip->disconnected))
		return;
	ret = usb_submit_urb(chip->state_urb, GFP_KERNEL);
	if (ret < 0 && ret != -EALREADY && chip->state_active)
		schedule_delayed_work(&chip->state_work, msecs_to_jiffies(10));
}

static void ff_meter_work_fn(struct work_struct *work)
{
	struct ff *chip = container_of(to_delayed_work(work), struct ff,
					 meter_work);
	int ret;

	if (!chip->state_active || READ_ONCE(chip->disconnected))
		return;
	ret = usb_submit_urb(chip->meter_urb, GFP_KERNEL);
	if (ret < 0 && ret != -EALREADY && chip->state_active)
		schedule_delayed_work(&chip->meter_work, msecs_to_jiffies(10));
}

static int ff_state_submit(struct ff *chip)
{
	int ret;

	if (!chip->state_urb)
		return 0;
	ret = usb_submit_urb(chip->state_urb, GFP_KERNEL);
	if (!ret)
		atomic_inc(&chip->state_urbs);
	return ret;
}

static int ff_meter_submit(struct ff *chip)
{
	int ret;

	if (!chip->meter_urb)
		return 0;
	ret = usb_submit_urb(chip->meter_urb, GFP_KERNEL);
	if (!ret)
		atomic_inc(&chip->meter_urbs);
	return ret;
}

int ff_transport_setup(struct ff *chip)
{
	const struct ff_device_desc *d = chip->desc;
	struct usb_host_interface *alt;
	unsigned int i;
	bool have_out = false, have_in = false, have_meter = false;

	if (d->state_iface == FF_IFACE_NONE)
		return 0;

	if (!chip->state_intf) {
		chip->state_intf = usb_ifnum_to_if(chip->dev, d->state_iface);
		if (!chip->state_intf)
			return -ENODEV;
		if (ff_claim_interface(chip->state_intf) < 0) {
			chip->state_intf = NULL;
			return -EBUSY;
		}
	}

	alt = chip->state_intf->cur_altsetting;
	for (i = 0; i < alt->desc.bNumEndpoints; i++) {
		struct usb_endpoint_descriptor *ep = &alt->endpoint[i].desc;

		if (usb_endpoint_is_bulk_out(ep) &&
		    ep->bEndpointAddress == d->state_out_ep)
			have_out = true;
		if (usb_endpoint_is_bulk_in(ep) &&
		    ep->bEndpointAddress == d->state_in_ep)
			have_in = true;
		if (usb_endpoint_is_bulk_in(ep) &&
		    ep->bEndpointAddress == d->meter_in_ep)
			have_meter = true;
	}

	if (!have_out && !have_in && !have_meter)
		return -ENODEV;

	if (!chip->transport_works_init) {
		INIT_DELAYED_WORK(&chip->state_work, ff_state_work_fn);
		INIT_DELAYED_WORK(&chip->meter_work, ff_meter_work_fn);
		chip->transport_works_init = true;
	}

	if (have_in && !chip->state_urb) {
		chip->state_urb = usb_alloc_urb(0, GFP_KERNEL);
		chip->state_buf = usb_alloc_coherent(chip->dev,
						     FF_STATE_BUF_SIZE,
						     GFP_KERNEL,
						     &chip->state_dma);
		if (chip->state_urb && chip->state_buf) {
			usb_fill_bulk_urb(chip->state_urb, chip->dev,
					  usb_rcvbulkpipe(chip->dev,
							  d->state_in_ep),
					  chip->state_buf, FF_STATE_BUF_SIZE,
					  ff_state_complete, chip);
			chip->state_urb->transfer_dma = chip->state_dma;
			chip->state_urb->transfer_flags |=
				URB_NO_TRANSFER_DMA_MAP;
		} else {
			usb_free_urb(chip->state_urb);
			chip->state_urb = NULL;
			if (chip->state_buf)
				usb_free_coherent(chip->dev,
						  FF_STATE_BUF_SIZE,
						  chip->state_buf,
						  chip->state_dma);
			chip->state_buf = NULL;
		}
	}
	if (have_meter && d->meter_len && !chip->meter_urb) {
		chip->meter_urb = usb_alloc_urb(0, GFP_KERNEL);
		/* Debug probe: allow one transfer to ask for two pages, to
		 * find out whether the endpoint delivers the heartbeat
		 * atomically.  The production length stays one page. */
		chip->meter_buf = usb_alloc_coherent(chip->dev,
						     2 * d->meter_len,
						     GFP_KERNEL,
						     &chip->meter_dma);
		if (chip->meter_urb && chip->meter_buf) {
			usb_fill_bulk_urb(chip->meter_urb, chip->dev,
					  usb_rcvbulkpipe(chip->dev,
							  d->meter_in_ep),
					  chip->meter_buf, d->meter_len,
					  ff_meter_complete, chip);
			chip->meter_urb->transfer_dma = chip->meter_dma;
			chip->meter_urb->transfer_flags |=
				URB_NO_TRANSFER_DMA_MAP;
		} else {
			usb_free_urb(chip->meter_urb);
			chip->meter_urb = NULL;
			if (chip->meter_buf)
				usb_free_coherent(chip->dev, 2 * d->meter_len,
						  chip->meter_buf,
						  chip->meter_dma);
			chip->meter_buf = NULL;
		}
	}

	dev_dbg(&chip->dev->dev,
		"raw control: IF%d ep out 0x%02x in 0x%02x meter 0x%02x\n",
		d->state_iface, d->state_out_ep, d->state_in_ep,
		d->meter_in_ep);
	if (atomic_read(&chip->hwdep_users))
		return ff_transport_start(chip);
	return 0;
}

int ff_transport_start(struct ff *chip)
{
	int ret;

	if (chip->state_active)
		return 0;
	chip->state_active = true;
	if (chip->caps & FF_TCAP_METER_PAIR) {
		chip->meter_pair_len = 0;
		WRITE_ONCE(chip->meter_paused, false);
	}
	/*
	 * The UC 0x85 endpoint carries the hardware Peak/RMS meter pages.
	 * Keep the meter URB running for both models; PCM capture remains
	 * owned by the ALSA/PipeWire stream.
	 */
	if (chip->caps & FF_TCAP_METER_PAIR) {
		ret = ff_meter_submit(chip);
		if (ret < 0 && ret != -EALREADY)
			dev_dbg(&chip->dev->dev, "UC meter URB submit: %d\n", ret);
		return 0;
	}
	ret = ff_state_submit(chip);
	if (ret < 0 && ret != -EALREADY)
		dev_dbg(&chip->dev->dev, "state URB submit: %d\n", ret);
	ret = ff_meter_submit(chip);
	if (ret < 0 && ret != -EALREADY)
		dev_dbg(&chip->dev->dev, "meter URB submit: %d\n", ret);
	return 0;
}

void ff_transport_stop(struct ff *chip)
{
	chip->state_active = false;
	if (chip->transport_works_init) {
		cancel_delayed_work_sync(&chip->state_work);
		cancel_delayed_work_sync(&chip->meter_work);
	}
	if (chip->state_urb)
		usb_kill_urb(chip->state_urb);
	if (chip->meter_urb)
		usb_kill_urb(chip->meter_urb);
	/*
	 * A heartbeat left half-read must be completed before the transport
	 * stops, or the device's page phase would shift for the next
	 * session.  Consume the partner page and discard the pair.
	 */
	if (chip->meter_pair_len && chip->desc &&
	    (chip->caps & FF_TCAP_METER_PAIR) &&
	    !READ_ONCE(chip->disconnected)) {
		u8 buf[FF_METER_PAIR_MAX];
		int actual = 0;
		int ret;

		ret = usb_bulk_msg(chip->dev,
				   usb_rcvbulkpipe(chip->dev,
						   chip->desc->meter_in_ep),
				   buf, chip->desc->meter_len, &actual, 50);
		if (ret || actual != (int)chip->desc->meter_len)
			chip->meter_phase_lost = true;
		chip->meter_pair_len = 0;
	} else if (chip->meter_pair_len) {
		chip->meter_phase_lost = true;
		chip->meter_pair_len = 0;
	}
	wake_up_interruptible_all(&chip->event_wait);
}

void ff_transport_suspend(struct ff *chip)
{
	ff_transport_stop(chip);
}

/* Quiesce the control transport at disconnect: stop the pump, kill the
 * state/meter URBs and release the interface. The event FIFO and the
 * URB buffers stay allocated until ff_hwdep_cleanup runs from the
 * card's private_free, after the last reader closes, so a reader inside
 * ff_hwdep_read never races a free.
 */
void ff_hwdep_quiesce(struct ff *chip)
{
	ff_transport_suspend(chip);
	if (chip->state_intf) {
		ff_release_interface(chip->state_intf);
		chip->state_intf = NULL;
	}
}

void ff_hwdep_cleanup(struct ff *chip)
{
	unsigned long flags;

	ff_transport_suspend(chip);
	if (chip->state_urb) {
		usb_free_urb(chip->state_urb);
		chip->state_urb = NULL;
	}
	if (chip->state_buf) {
		usb_free_coherent(chip->dev, FF_STATE_BUF_SIZE,
				  chip->state_buf, chip->state_dma);
		chip->state_buf = NULL;
	}
	if (chip->meter_urb) {
		usb_free_urb(chip->meter_urb);
		chip->meter_urb = NULL;
	}
	if (chip->meter_buf) {
		usb_free_coherent(chip->dev, 2 * chip->desc->meter_len,
				  chip->meter_buf, chip->meter_dma);
		chip->meter_buf = NULL;
	}
	spin_lock_irqsave(&chip->event_lock, flags);
	if (kfifo_initialized(&chip->event_fifo)) {
		kfifo_free(&chip->event_fifo);
		memset(&chip->event_fifo, 0, sizeof(chip->event_fifo));
	}
	spin_unlock_irqrestore(&chip->event_lock, flags);
	chip->hwdep = NULL;
}
