// SPDX-License-Identifier: GPL-2.0-only
/*
 * RME Fireface USB - kernel transport core.
 *
 * This file owns USB enumeration, the streaming engines, the ALSA PCM
 * device and the per-device descriptor table.  Device-model policy lives in
 * userspace behind the snd-hwdep node created by ffuc-hwdep.c; see
 * ffuc.h and docs/PROTOCOL-COMPLETE.md.
 */
#include <linux/bitops.h>
#include <linux/debugfs.h>
#include <linux/ktime.h>
#include <linux/math64.h>
#include <linux/module.h>
#include <linux/slab.h>
#include <linux/string.h>
#include <linux/usb.h>
#include <sound/core.h>
#include <sound/initval.h>
#include <sound/pcm.h>
#include <sound/pcm_params.h>

#include "ffuc.h"

static int index[SNDRV_CARDS] = SNDRV_DEFAULT_IDX;
static char *id[SNDRV_CARDS] = SNDRV_DEFAULT_STR;
static bool enable[SNDRV_CARDS] = SNDRV_DEFAULT_ENABLE_PNP;
static unsigned int urb_frames = 48;
static unsigned int nurbs = 2;
static bool ucx_if5;
static unsigned int ucx_if5_frames = 1024;
static unsigned int fixed_rate;
/* q48 instrumentation (test/uc-q48-instrumentation).  Poison the PCM rings
 * with 0x00005A5A instead of the inherited memory so that a reader's
 * preserved zero can be distinguished from never-written memory.  Values
 * are near-silent in both formats: float32 denormal and s32 DC null. */
static bool ffuc_poison = true;

static DEFINE_MUTEX(devices_mutex);
static unsigned long devices_used;

static struct usb_driver ffuc_driver;

module_param(ffuc_poison, bool, 0644);
MODULE_PARM_DESC(ffuc_poison, "Poison-fill the PCM rings with 0x00005A5A at prepare (diagnostics).");

module_param_array(index, int, NULL, 0444);
MODULE_PARM_DESC(index, "Index value for the Fireface sound card.");
module_param_array(id, charp, NULL, 0444);
MODULE_PARM_DESC(id, "ID string for the Fireface sound card.");
module_param_array(enable, bool, NULL, 0444);
MODULE_PARM_DESC(enable, "Enable the Fireface sound card.");
module_param(urb_frames, uint, 0444);
MODULE_PARM_DESC(urb_frames, "Frames per URB at 32-48 kHz, multiple of 8 (8..256); doubled at 2x and quadrupled at 4x on the interrupt transport.");
module_param(nurbs, uint, 0444);
MODULE_PARM_DESC(nurbs, "URBs in flight per endpoint (2..32, default 2); raised so at least 64 frames stay queued.");
module_param(ucx_if5, bool, 0444);
MODULE_PARM_DESC(ucx_if5, "Fireface UCX: bind the Windows-style interrupt interface 5 (default false). The default binds the Mac-style ISO interface 0; set Y only for the Windows transport.");
module_param(ucx_if5_frames, uint, 0444);
MODULE_PARM_DESC(ucx_if5_frames, "Fireface UCX interrupt transport: frames per URB at 1x rates (8..1024, default 1024).");
module_param(fixed_rate, uint, 0644);
MODULE_PARM_DESC(fixed_rate, "Only allow this sample rate (0 = any supported rate).");

/* -- sample rates and bandwidth classes ----------------------------- */

static const struct ffuc_mode ffuc_pc_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .pkt_frames = 8 },	/* 1x */
	{ .alt = 2, .neps = 1, .words = 14, .pkt_frames = 16 },	/* 2x */
	{ .alt = 3, .neps = 2, .words = 6, .pkt_frames = 32 },	/* 4x */
};

/* UCX interrupt transport (interface 5).  The Windows reference host
 * selects alt 1 once at connect and streams 44.1/48 kHz on it (E2b/E5,
 * win-20261027-ucx-transport).  Alt 2 carries 2x (14 words, ADAT SMUX);
 * alt 3 is the 4x path with two capture endpoints.  The 2x/4x alternate
 * selection awaits a capture. */
static const struct ffuc_mode ffuc_ucx_if5_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .pkt_frames = 8 },
	{ .alt = 2, .neps = 1, .words = 14, .pkt_frames = 16 },
	{ .alt = 3, .neps = 2, .words = 6, .pkt_frames = 32 },
};

/* ISO transport (UC Mac personality and UCX interface 0).  The packet
 * maxima are wMaxPacketSize * mult from the descriptors; the UCX OUT
 * packet adds one lead word per frame. */
static const struct ffuc_mode ffuc_mac_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .in_mps = 508, .out_mps = 532, .max_frames = 7 },
	{ .alt = 2, .neps = 1, .words = 14, .in_mps = 732, .out_mps = 780, .max_frames = 13 },
	{ .alt = 3, .neps = 1, .words = 12, .in_mps = 1208, .out_mps = 1304, .max_frames = 25 },
};

static const struct ffuc_mode ffuc_ucx_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .in_mps = 508, .out_mps = 532, .max_frames = 7 },
	{ .alt = 2, .neps = 1, .words = 14, .in_mps = 732, .out_mps = 780, .max_frames = 13 },
	{ .alt = 3, .neps = 1, .words = 12, .in_mps = 1208, .out_mps = 1304, .max_frames = 25 },
};

/* Babyface-classic ISO interface 0.  Same packet format as the UCX; the
 * counts follow from wMaxPacketSize: IN = 4 + frames * words * 4,
 * OUT = frames * (1 + out_words) * 4. */
static const struct ffuc_mode ffuc_babyface_classic_modes[] = {
	{ .alt = 1, .neps = 1, .words = 12, .out_words = 14, .in_mps = 340, .out_mps = 420, .max_frames = 7 },
	{ .alt = 2, .neps = 1, .words = 8, .out_words = 10, .in_mps = 420, .out_mps = 572, .max_frames = 13 },
	{ .alt = 3, .neps = 1, .words = 6, .out_words = 8, .in_mps = 604, .out_mps = 900, .max_frames = 25 },
};

static const unsigned int ffuc_rates[] = {
	32000, 44100, 48000, 64000, 88200, 96000, 128000, 176400, 192000,
	352800, 384000, 705600, 768000,
};

static const struct snd_pcm_hw_constraint_list ffuc_rate_list = {
	.count = ARRAY_SIZE(ffuc_rates),
	.list = ffuc_rates,
};

static unsigned int ffuc_mode_index(unsigned int rate)
{
	if (rate <= 48000)
		return 0;
	if (rate <= 96000)
		return 1;
	return 2;
}

static bool ffuc_rate_supported(struct ffuc *chip, unsigned int rate)
{
	int i;

	if (rate > chip->desc->max_rate)
		return false;
	for (i = 0; i < ARRAY_SIZE(ffuc_rates); i++)
		if (ffuc_rates[i] == rate)
			return true;
	return false;
}

static unsigned int ffuc_mode_channels(struct ffuc *chip, unsigned int rate)
{
	const struct ffuc_mode *m = &chip->modes[ffuc_mode_index(rate)];

	/* Playback words: the stream open/close words mute and unmute
	 * playback channels. */
	return m->neps * ffuc_mode_out_words(m);
}

/*
 * The ALSA PCM keeps a constant logical channel count.  At 2x/4x the USB
 * packet carries fewer words (ADAT S/MUX); the engine maps the first
 * 14/12 words and the remaining channels stay silent.  This lets PipeWire
 * keep one 18-channel node while the card runs at every supported rate.
 */
static unsigned int ffuc_max_channels(struct ffuc *chip, int stream,
				      unsigned int rate)
{
	(void)rate;
	return stream == SNDRV_PCM_STREAM_PLAYBACK ?
	       ffuc_desc_playback(chip->desc) : chip->desc->max_channels;
}

static unsigned int ffuc_max_words(struct ffuc *chip)
{
	unsigned int i, n = 0;

	for (i = 0; i < chip->desc->n_modes; i++) {
		const struct ffuc_mode *m = &chip->modes[i];

		n = max(n, m->neps * max(m->words, ffuc_mode_out_words(m)));
	}
	return n;
}

static unsigned int ffuc_urb_frames(struct ffuc *chip, unsigned int rate)
{
	if (chip->iso)
		return chip->iso_packets * DIV_ROUND_UP(rate, 8000);
	/* The Windows IF5 driver uses one fixed URB size at every rate. */
	if (chip->ucx)
		return chip->base_urb_frames;
	return chip->base_urb_frames << ffuc_mode_index(rate);
}

/* AP-mode ISO header layout per rate band (1x/2x/4x).  The position field
 * widens with the band (bits 5..17/18/19); the phase field is FIXED at
 * bits 21..31 (11-bit) at every band, above the widened position.  The
 * layout is generic across AP models (UC, UCX, Babyface-classic share it);
 * only the rate band selects the position width.  The phase VALUE wraps at
 * tps = 60000000/rate (a 60 MHz tick), so the fractional-position law
 * phase/tps is band-independent. */
struct ffuc_iso_layout {
	u32 pos_mask;
	u32 modulus;
	unsigned int phase_shift;
	u32 phase_mask;
};

static const struct ffuc_iso_layout ffuc_iso_layouts[3] = {
	{ 0x1fff, 0x2000, 21, 0x7ff },	/* 1x: pos 5..17, phase 21..31 */
	{ 0x3fff, 0x4000, 21, 0x7ff },	/* 2x: pos 5..18, phase 21..31 */
	{ 0x7fff, 0x8000, 21, 0x7ff },	/* 4x: pos 5..19, phase 21..31 */
};

static const struct ffuc_iso_layout *ffuc_iso_layout(struct ffuc *chip)
{
	return &ffuc_iso_layouts[ffuc_mode_index(chip->rate)];
}

static u32 ffuc_iso_pos_mask(struct ffuc *chip)
{
	return ffuc_iso_layout(chip)->pos_mask;
}

static unsigned int ffuc_iso_phase_shift(struct ffuc *chip)
{
	return ffuc_iso_layout(chip)->phase_shift;
}

static u32 ffuc_iso_phase_mask(struct ffuc *chip)
{
	return ffuc_iso_layout(chip)->phase_mask;
}

unsigned int ffuc_iso_wrap(struct ffuc *chip)
{
	/* The position counter modulus widens with the rate band: 8192 at
	 * 1x, 16384 at 2x, 32768 at 4x.  It is a power of two, so the field
	 * mask and the modulo reduction agree. */
	return ffuc_iso_layout(chip)->modulus;
}

unsigned int ffuc_rate_from_index(unsigned int idx)
{
	static const unsigned int map[12] = {
		32000, 44100, 48000, 48000, 64000, 88200,
		96000, 96000, 128000, 176400, 192000, 192000,
	};

	return idx < ARRAY_SIZE(map) ? map[idx] : 0;
}

/* -- vendor requests ------------------------------------------------- */

int ffuc_vendor_send(struct ffuc *chip, u8 req, u16 val, u16 idx,
		     const void *data, u16 len)
{
	if (READ_ONCE(chip->disconnected))
		return -ENODEV;
	return usb_control_msg_send(chip->dev, 0, req,
				    USB_DIR_OUT | USB_TYPE_VENDOR |
				    USB_RECIP_DEVICE, val, idx,
				    (void *)data, len, FFUC_CTL_TIMEOUT,
				    GFP_KERNEL);
}

int ffuc_vendor_recv(struct ffuc *chip, u8 req, u16 val, u16 idx,
		     void *data, u16 len)
{
	if (READ_ONCE(chip->disconnected))
		return -ENODEV;
	return usb_control_msg_recv(chip->dev, 0, req,
				    USB_DIR_IN | USB_TYPE_VENDOR |
				    USB_RECIP_DEVICE, val, idx,
				    data, len, FFUC_CTL_TIMEOUT, GFP_KERNEL);
}

/* One IF1 register word: [parity:1][addr:15][value:16], little-endian. */
static u32 ffuc_word_encode(u16 addr, u16 value)
{
	u32 w = ((u32)(addr & 0x7fff) << 16) | value;
	u32 p = w;

	p ^= p >> 16;
	p ^= p >> 8;
	p ^= p >> 4;
	p ^= p >> 2;
	p ^= p >> 1;
	if (!(p & 1))
		w |= 0x80000000u;
	return w;
}

int ffuc_reg_send(struct ffuc *chip, const struct ffuc_hwdep_word *words,
		  unsigned int count)
{
	u8 *buf;
	unsigned int done = 0;
	int ret = 0;

	if (!chip->desc->state_out_ep)
		return -ENODEV;
	if (!count)
		return 0;
	if (count > FFUC_HWDEP_MAX_WORDS)
		count = FFUC_HWDEP_MAX_WORDS;

	buf = kmalloc(count * sizeof(u32), GFP_KERNEL);
	if (!buf)
		return -ENOMEM;

	while (done < count) {
		unsigned int n = min_t(unsigned int, count - done,
				       FFUC_HWDEP_MAX_WORDS);
		unsigned int i, actual;
		u8 *p = buf;

		for (i = 0; i < n; i++) {
			u32 w = ffuc_word_encode(words[done + i].addr,
						 words[done + i].value);

			*p++ = w;
			*p++ = w >> 8;
			*p++ = w >> 16;
			*p++ = w >> 24;
		}
		mutex_lock(&chip->ctl_lock);
		ret = usb_bulk_msg(chip->dev,
				   usb_sndbulkpipe(chip->dev,
						   chip->desc->state_out_ep),
				   buf, n * sizeof(u32), &actual, 100);
		mutex_unlock(&chip->ctl_lock);
		if (ret < 0)
			break;
		done += n;
	}
	kfree(buf);
	return ret;
}

/* Claim/release a sibling interface for the raw control transports.  The
 * USB driver itself only probes the streaming interface, so the meter/state
 * interface must be claimed explicitly.  The helpers keep ffuc_driver local
 * to this file while ffuc-hwdep.c can still manage the interface. */
int ffuc_claim_interface(struct usb_interface *intf)
{
	return usb_driver_claim_interface(&ffuc_driver, intf, NULL);
}

void ffuc_release_interface(struct usb_interface *intf)
{
	usb_driver_release_interface(&ffuc_driver, intf);
}

/* -- per-device core sequences --------------------------------------- */

/*
 * UC channel initialization.  The 0x16 wValue is the per-channel sample
 * format: 0 = S32, 1 = float32.  It must equal the wire format the host
 * sends (ffuc_wire_s32); a mismatch passes only the sample sign bit and
 * distorts the audio.  The value is therefore derived from the resolved
 * wire format, not stored per range.  Both UC personalities use the
 * 36-slot init (18 inputs + 18 outputs); the interrupt personality pairs
 * it with S32 and the ISO personality with float32.  The Windows host
 * instead writes a 60-slot block with S32 (documented in the protocol
 * spec); the slot count does not affect the format pairing.
 */
static const struct ffuc_init_range ffuc_uc_chan_init[] = {
	{ .first = 0,    .count = 18 },
	{ .first = 0x20, .count = 18 },
};

static const struct ffuc_init_range ffuc_ucx_chan_init[] = {
	{ .first = 0,    .count = FFUC_MAX_CHANNELS },
	{ .first = 0x20, .count = FFUC_MAX_CHANNELS },
};

/* UCX interrupt init covers 60 slots: 0..29 and 32..61 (E2b capture). */
static const struct ffuc_init_range ffuc_ucx_if5_chan_init[] = {
	{ .first = 0,    .count = 30 },
	{ .first = 0x20, .count = 30 },
};

/* Write the descriptor's 0x16 slot ranges.  The wValue is the channel
 * sample format and follows the resolved wire format (0 = S32,
 * 1 = float32). */
static int ffuc_uc_chan_init_write(struct ffuc *chip)
{
	const struct ffuc_init_range *range;
	u16 value = chip->s32_samples ? 0x0000 : 0x0001;
	unsigned int i, r;
	int ret;

	for (r = 0; r < chip->desc->n_chan_init; r++) {
		range = &chip->desc->chan_init[r];
		for (i = 0; i < range->count; i++) {
			ret = ffuc_vendor_send(chip, FFUC_REQ_CHAN_INIT,
					       value, range->first + i, NULL, 0);
			if (ret < 0)
				return ret;
		}
	}
	return 0;
}

/* UC init: 36 channel slots, then the settings word and the global enable.
 * Used by both UC personalities; the wire format follows the transport
 * rule (S32 interrupt, float32 ISO). */
static int ffuc_uc_init(struct ffuc *chip)
{
	int ret;

	ret = ffuc_uc_chan_init_write(chip);
	if (ret < 0)
		return ret;
	ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS, 0x0800, 0xb800,
			       NULL, 0);
	if (ret < 0)
		return ret;
	return ffuc_vendor_send(chip, FFUC_REQ_ENABLE, 0x0001, 0x0000,
				NULL, 0);
}

static unsigned int ffuc_rate_family(unsigned int rate)
{
	switch (rate) {
	case 32000:
	case 64000:
	case 128000:
		return 0;
	case 44100:
	case 88200:
	case 176400:
		return 1;
	default:
		return 2;
	}
}

static int ffuc_set_rate_family(struct ffuc *chip, unsigned int rate)
{
	return ffuc_vendor_send(chip, FFUC_REQ_SETTINGS,
				ffuc_rate_family(rate) << 4, 0x0030, NULL, 0);
}

/* The rate sequence is one generic flow for every model and transport:
 * the DDS divisor word, the family select, then the options commit.
 * The DDS word carries base/rate with the 2x/4x shift; the family word
 * selects the base-rate family at index 0x30.  The commit applies the
 * change and rewrites the options word the device already holds, so the
 * mixer's settings (clock source, word clock, SPDIF and optical output
 * format) survive the rate change. */
static int ffuc_set_rate(struct ffuc *chip, unsigned int rate)
{
	__le32 cur;
	u16 opts;
	u32 dds;
	int ret;

	dds = (u32)div_u64(0x5f5e10000000ULL, rate);
	if (rate > 114000)
		dds <<= 2;
	else if (rate > 57000)
		dds <<= 1;

	ret = ffuc_vendor_send(chip, FFUC_REQ_DDS, dds >> 16,
			       (dds & 0xff00) | ffuc_rate_family(rate),
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ffuc_set_rate_family(chip, rate);
	if (ret < 0)
		return ret;
	ret = ffuc_vendor_recv(chip, FFUC_REQ_SETTINGS, 0, 0, &cur,
			       sizeof(cur));
	if (ret < 0)
		return ret;
	opts = le32_to_cpu(cur) & FFUC_OPTIONS_MASK;
	ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS, opts,
			       FFUC_OPTIONS_MASK, NULL, 0);
	dev_dbg(&chip->dev->dev,
		"rate %u Hz: dds 0x%04x/0x%04x family 0x%02x commit 0x%04x ret=%d\n",
		rate, dds >> 16, (dds & 0xff00) | ffuc_rate_family(rate),
		ffuc_rate_family(rate) << 4, opts, ret);
	return ret;
}

static int ffuc_uc_hw_mute(struct ffuc *chip, bool mute)
{
	return ffuc_vendor_send(chip,
				mute ? FFUC_REQ_MUTE : FFUC_REQ_UNMUTE,
				0xffff, 0xc000, NULL, 0);
}

/* Requests 19/20 mute/unmute playback channels by mask.  UC/UCX: channel
 * n is bit n + 2, high half in wValue, low half in wIndex.
 * Babyface-classic: see below. */
static int ffuc_playback_mask(struct ffuc *chip, bool mute,
			      unsigned int channels)
{
	u32 mask;
	u16 idx, val;

	if (chip->desc->model == FFUC_MODEL_BABYFACE_CLASSIC) {
		/* The Babyface-classic reads an 18-bit mask packed like a
		 * 0x12 level: channel n is bit n, wValue = mask >> 2,
		 * wIndex bits 14-15 = mask bits 0-1.  The vendor driver
		 * sends all 18 bits. */
		mask = (1u << FFUC_MAX_CHANNELS) - 1;
		val = mask >> 2;
		idx = (mask & 3) << 14;
	} else {
		if (channels > 30)
			channels = 30;
		mask = ((1u << channels) - 1) << 2;
		val = mask >> 16;
		idx = mask & 0xffff;
	}
	return ffuc_vendor_send(chip,
				mute ? FFUC_REQ_UCX_MUTE :
				       FFUC_REQ_UCX_UNMUTE,
				val, idx, NULL, 0);
}

static int ffuc_ucx_hw_mute(struct ffuc *chip, bool mute)
{
	return ffuc_playback_mask(chip, mute, FFUC_MAX_CHANNELS);
}

static int ffuc_ucx_stream_start(struct ffuc *chip, unsigned int channels)
{
	int ret;

	/* The Windows driver sends these immediately before the first
	 * audio packet.  0x8000/0x8000 is required for the DACs to open on
	 * the interrupt transport; 0x1d is not understood but is part of the
	 * captured working sequence. */
	ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS, 0x8000, 0x8000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ffuc_vendor_send(chip, FFUC_REQ_UCX_STREAM29, 0x0000, 0x0000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	return ffuc_playback_mask(chip, false, channels);
}

static int ffuc_ucx_stream_stop(struct ffuc *chip, unsigned int channels)
{
	return ffuc_playback_mask(chip, true, channels);
}

static int ffuc_uc_stream_start(struct ffuc *chip, unsigned int channels)
{
	int ret;

	(void)channels;
	if (!chip->uc_session)
		return 0;
	dev_dbg(&chip->dev->dev,
		"UC session open (rate=%u channels=%u session_reopen=%d)\n",
		chip->rate, channels, chip->session_reopen);

	/* Windows session-open sequence, measured in A2b on every stream
	 * open:
	 *   0x10  wIndex 0x8000  wValue 0x0000
	 *   0x1D  wIndex 0       wValue 0x0000
	 *   0x14  wIndex 0xC000  wValue 0x000F
	 * The UC needs it after a rate change; the initial 48 kHz path uses
	 * the probe/init sequence and must stay untouched.  Do not confuse
	 * wValue and wIndex: 0x10 carries 0x8000 in wIndex and zero in
	 * wValue. */
	ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS, 0x0000, 0x8000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ffuc_vendor_send(chip, FFUC_REQ_UCX_STREAM29, 0x0000, 0x0000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ffuc_vendor_send(chip, FFUC_REQ_UNMUTE, 0x000f, 0xc000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	chip->session_reopen = false;
	return 0;
}

static int ffuc_uc_stream_stop(struct ffuc *chip, unsigned int channels)
{
	(void)channels;
	/* Windows closes the session before every rate write (A2 t=13.03,
	 * rate at 13.25; FP9c t=4.88, rate at 10.48).  This is not gated on
	 * a previous rate change: the first 48 -> 44.1 switch needs it. */
	dev_dbg(&chip->dev->dev,
		"UC session close (rate=%u channels=%u)\n",
		chip->rate, channels);
	/* Windows session-close word: 0x13 wIndex 0xC000 wValue 0x000F. */
	return ffuc_vendor_send(chip, FFUC_REQ_MUTE, 0x000f, 0xc000,
				NULL, 0);
}

static const struct ffuc_ops ffuc_uc_ops = {
	.init = ffuc_uc_init,
	.set_rate = ffuc_set_rate,
	.stream_start = ffuc_uc_stream_start,
	.stream_stop = ffuc_uc_stream_stop,
	.hw_mute = ffuc_uc_hw_mute,
};

static const struct ffuc_ops ffuc_ucx_ops = {
	.init = ffuc_uc_init,
	.set_rate = ffuc_set_rate,
	.stream_start = ffuc_ucx_stream_start,
	.stream_stop = ffuc_ucx_stream_stop,
	.hw_mute = ffuc_ucx_hw_mute,
};

/* UCX interrupt (IF5) connect init, captured from the Windows driver
 * (E2b replug, win-20261027-ucx-transport): the 60-slot channel init,
 * the DDS/clock quad, the gain-ref word and the settings group.  The
 * quad is written once at connect; a rate change writes the family word
 * alone.  The settings group carries a ~100 ms settle between the
 * 0x3000 writes, measured in the capture. */
static int ffuc_ucx_if5_init(struct ffuc *chip)
{
	static const u16 dds[4][2] = {
		{ 0xc350, 0x0000 }, { 0x8db9, 0x2d01 },
		{ 0x8235, 0x5502 }, { 0x7d00, 0x0003 },
	};
	static const u16 grp_pre[5][2] = {
		{ 0x0000, 0x3000 }, { 0x0000, 0x3000 },
		{ 0x0800, 0x0800 }, { 0x0000, 0x0800 },
		{ 0x2000, 0x3000 },
	};
	static const u16 grp_post[6][2] = {
		{ 0x0000, 0x3000 },
		{ 0x0800, 0x0800 }, { 0x0800, 0x0800 },
		{ 0x0800, 0x0800 }, { 0x0800, 0x0800 },
		{ 0x0800, 0x0800 },
	};
	int ret, i;

	ret = ffuc_uc_chan_init_write(chip);
	if (ret < 0)
		return ret;
	for (i = 0; i < 4; i++) {
		ret = ffuc_vendor_send(chip, FFUC_REQ_DDS, dds[i][0],
				       dds[i][1], NULL, 0);
		if (ret < 0)
			return ret;
	}
	ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS, 0x0028, 0x05ff,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ffuc_vendor_send(chip, FFUC_REQ_LEVELS, 0x3c00, 0x0fc0,
			       NULL, 0);
	if (ret < 0)
		return ret;
	for (i = 0; i < 5; i++) {
		ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS, grp_pre[i][0],
				       grp_pre[i][1], NULL, 0);
		if (ret < 0)
			return ret;
	}
	msleep(FFUC_UCX_IF5_SETTLE_MS);
	for (i = 0; i < 6; i++) {
		ret = ffuc_vendor_send(chip, FFUC_REQ_SETTINGS,
				       grp_post[i][0], grp_post[i][1],
				       NULL, 0);
		if (ret < 0)
			return ret;
	}
	return 0;
}

static const struct ffuc_ops ffuc_ucx_if5_ops = {
	.init = ffuc_ucx_if5_init,
	.set_rate = ffuc_set_rate,
	.stream_start = ffuc_ucx_stream_start,
	.stream_stop = ffuc_ucx_stream_stop,
	.hw_mute = ffuc_ucx_hw_mute,
};

/* Resolve the wire sample format once at probe.  The format is keyed to
 * the transport: the interrupt personalities carry S32 (the Windows
 * reference pairing) and the ISO personalities carry float32 (the macOS
 * reference pairing).  The device accepts either format when the 0x16
 * channel-init value matches; ffuc_uc_chan_init_write derives that value
 * from this decision so the two can never mismatch. */
static bool ffuc_wire_s32(const struct ffuc *chip)
{
	return !chip->iso;
}

/* -- per-device descriptors ------------------------------------------ */

/* Named indices into ffuc_devices[].  The USB ID table references these
 * instead of raw positions, and the count assertion after the table fails
 * the build when a descriptor is added without updating the enum.  Append
 * new devices at the end of both. */
enum {
	FFUC_DEV_UC_PC,
	FFUC_DEV_UC_MAC_SMSC,
	FFUC_DEV_UC_MAC_RME,
	FFUC_DEV_UCX_ISO_SMSC,
	FFUC_DEV_UCX_IF5_SMSC,
	FFUC_DEV_UCX_ISO_RME,
	FFUC_DEV_UCX_IF5_RME,
	FFUC_DEV_BABYFACE_CLASSIC,
	FFUC_DEV_UC_WIN_RME,
	FFUC_DEV_COUNT
};

static const struct ffuc_device_desc ffuc_devices[] = {
	{
		.name = "Fireface UC",
		.shortname = "Fireface UC",
		.card_id = "FirefaceUC",
		.vendor = FFUC_VENDOR_SMSC,
		.product = FFUC_PRODUCT_UC_PC,
		.iface = 0,
		.transport = FFUC_XPORT_INTERRUPT,
		.quirks = FFUC_QUIRK_STATUS_LATCH | FFUC_QUIRK_WIN_SESSION,
		.model = FFUC_MODEL_UC,
		.modes = ffuc_pc_modes,
		.n_modes = ARRAY_SIZE(ffuc_pc_modes),
		.audio_iface = 0,
		.out_ep = 0x01,
		.in_ep = 0x02,
		.status_iface = 5,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.midi_ports = 2,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 324,
		.chan_init = ffuc_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_uc_chan_init),
		.ops = &ffuc_uc_ops,
	}, {
		.name = "Fireface UC",
		.shortname = "Fireface UC",
		.card_id = "FirefaceUC",
		.vendor = FFUC_VENDOR_SMSC,
		.product = FFUC_PRODUCT_UC_MAC,
		.iface = 0,
		.transport = FFUC_XPORT_ISO,
		.quirks = 0,
		.model = FFUC_MODEL_UC,
		.modes = ffuc_mac_modes,
		.n_modes = ARRAY_SIZE(ffuc_mac_modes),
		.audio_iface = 0,
		.out_ep = 0x01,
		.in_ep = 0x02,
		.status_iface = 4,
		.status_bulk = 1,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 324,
		.chan_init = ffuc_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_uc_chan_init),
		.ops = &ffuc_uc_ops,
	}, {
		.name = "Fireface UC",
		.shortname = "Fireface UC",
		.card_id = "FirefaceUC",
		.vendor = FFUC_VENDOR_RME,
		.product = FFUC_PRODUCT_UC_MAC,
		.iface = 0,
		.transport = FFUC_XPORT_ISO,
		.quirks = 0,
		.model = FFUC_MODEL_UC,
		.modes = ffuc_mac_modes,
		.n_modes = ARRAY_SIZE(ffuc_mac_modes),
		.audio_iface = 0,
		.out_ep = 0x01,
		.in_ep = 0x02,
		.status_iface = 4,
		.status_bulk = 1,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 324,
		.chan_init = ffuc_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_uc_chan_init),
		.ops = &ffuc_uc_ops,
	}, {
		.name = "Fireface UCX",
		.shortname = "Fireface UCX",
		.card_id = "FirefaceUCX",
		.vendor = FFUC_VENDOR_SMSC,
		.product = FFUC_PRODUCT_UCX,
		.iface = 0,
		.transport = FFUC_XPORT_ISO,
		.quirks = 0,
		.model = FFUC_MODEL_UCX,
		.modes = ffuc_ucx_modes,
		.n_modes = ARRAY_SIZE(ffuc_ucx_modes),
		.audio_iface = 0,
		.out_ep = 0x0e,
		.in_ep = 0x0f,
		.status_iface = 4,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.midi_ports = 2,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 244,
		.chan_init = ffuc_ucx_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_ucx_chan_init),
		.ops = &ffuc_ucx_ops,
	}, {
		.name = "Fireface UCX",
		.shortname = "Fireface UCX",
		.card_id = "FirefaceUCX",
		.vendor = FFUC_VENDOR_SMSC,
		.product = FFUC_PRODUCT_UCX,
		.iface = 5,
		.transport = FFUC_XPORT_INTERRUPT,
		.quirks = FFUC_QUIRK_ALT_RESELECT |
			  FFUC_QUIRK_IF5_PRE_START |
			  FFUC_QUIRK_IF5_PLAYBACK_MASK,
		.model = FFUC_MODEL_UCX,
		.modes = ffuc_ucx_if5_modes,
		.n_modes = ARRAY_SIZE(ffuc_ucx_if5_modes),
		.audio_iface = 5,
		.out_ep = 0x01,
		.in_ep = 0x02,
		.status_iface = 4,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.midi_ports = 2,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 244,
		.chan_init = ffuc_ucx_if5_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_ucx_if5_chan_init),
		.ops = &ffuc_ucx_if5_ops,
	}, {
		.name = "Fireface UCX",
		.shortname = "Fireface UCX",
		.card_id = "FirefaceUCX",
		.vendor = FFUC_VENDOR_RME,
		.product = FFUC_PRODUCT_UCX,
		.iface = 0,
		.transport = FFUC_XPORT_ISO,
		.quirks = 0,
		.model = FFUC_MODEL_UCX,
		.modes = ffuc_ucx_modes,
		.n_modes = ARRAY_SIZE(ffuc_ucx_modes),
		.audio_iface = 0,
		.out_ep = 0x0e,
		.in_ep = 0x0f,
		.status_iface = 4,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.midi_ports = 2,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 244,
		.chan_init = ffuc_ucx_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_ucx_chan_init),
		.ops = &ffuc_ucx_ops,
	}, {
		.name = "Fireface UCX",
		.shortname = "Fireface UCX",
		.card_id = "FirefaceUCX",
		.vendor = FFUC_VENDOR_RME,
		.product = FFUC_PRODUCT_UCX,
		.iface = 5,
		.transport = FFUC_XPORT_INTERRUPT,
		.quirks = FFUC_QUIRK_ALT_RESELECT |
			  FFUC_QUIRK_IF5_PRE_START |
			  FFUC_QUIRK_IF5_PLAYBACK_MASK,
		.model = FFUC_MODEL_UCX,
		.modes = ffuc_ucx_if5_modes,
		.n_modes = ARRAY_SIZE(ffuc_ucx_if5_modes),
		.audio_iface = 5,
		.out_ep = 0x01,
		.in_ep = 0x02,
		.status_iface = 4,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.midi_ports = 2,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 244,
		.chan_init = ffuc_ucx_if5_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_ucx_if5_chan_init),
		.ops = &ffuc_ucx_if5_ops,
	}, {
		/* ISO audio, MIDI and the IF1 0x85 DSP meter read-back.  The
		 * IF1 bulk OUT 0x0a carries EQ blocks and is not exposed yet;
		 * the roles of interfaces 3 (0x09/0x88) and 4 (0x8b) are not
		 * mapped.  The meter packet length is not known, so the URB
		 * takes a full 512-byte bulk packet and passes the actual
		 * length on. */
		.name = "Babyface-classic",
		.shortname = "Babyface-classic",
		.card_id = "Babyface",
		.vendor = FFUC_VENDOR_RME,
		.product = FFUC_PRODUCT_BABYFACE_CLASSIC,
		.iface = 0,
		.transport = FFUC_XPORT_ISO,
		.quirks = 0,
		.model = FFUC_MODEL_BABYFACE_CLASSIC,
		.modes = ffuc_babyface_classic_modes,
		.n_modes = ARRAY_SIZE(ffuc_babyface_classic_modes),
		.audio_iface = 0,
		.out_ep = 0x03,
		.in_ep = 0x84,
		.status_iface = FFUC_IFACE_NONE,
		.state_iface = 1,
		.meter_in_ep = 0x85,
		.meter_len = 512,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.max_channels = 12,
		.max_playback = 14,
		.max_rate = 192000,
		.ops = &ffuc_ucx_ops,
	}, {
		.name = "Fireface UC",
		.shortname = "Fireface UC",
		.card_id = "FirefaceUC",
		.vendor = FFUC_VENDOR_RME,
		.product = FFUC_PRODUCT_UC_WIN_RME,
		.iface = 0,
		.transport = FFUC_XPORT_INTERRUPT,
		/* The output latch clears only on an alt 0 -> alt N reselect,
		 * as on the UCX.  The wire format follows the transport rule
		 * (S32 on the interrupt personality). */
		.quirks = FFUC_QUIRK_STATUS_LATCH |
			  FFUC_QUIRK_ALT_RESELECT |
			  FFUC_QUIRK_WIN_SESSION,
		.model = FFUC_MODEL_UC,
		.modes = ffuc_pc_modes,
		.n_modes = ARRAY_SIZE(ffuc_pc_modes),
		.audio_iface = 0,
		.out_ep = 0x01,
		.in_ep = 0x02,
		.status_iface = 5,
		.status_ep = 0x8b,
		.state_iface = 1,
		.state_out_ep = 0x0c,
		.state_in_ep = 0x8d,
		.meter_in_ep = 0x85,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.midi_ports = 2,
		.max_channels = 18,
		.max_rate = 192000,
		.meter_len = 324,
		.chan_init = ffuc_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ffuc_uc_chan_init),
		.ops = &ffuc_uc_ops,
	},
};

/* The ID table indexes ffuc_devices by the named enum above; a new
 * descriptor without a new enum entry (or vice versa) fails the build. */
static_assert(ARRAY_SIZE(ffuc_devices) == FFUC_DEV_COUNT);

static const struct usb_device_id ffuc_ids[] = {
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_SMSC, FFUC_PRODUCT_UC_PC, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UC_PC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_RME, FFUC_PRODUCT_UC_PC_RME, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UC_PC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_RME, FFUC_PRODUCT_UC_WIN_RME, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UC_WIN_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_SMSC, FFUC_PRODUCT_UC_MAC, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UC_MAC_SMSC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_RME, FFUC_PRODUCT_UC_MAC, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UC_MAC_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_SMSC, FFUC_PRODUCT_UCX, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UCX_ISO_SMSC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_SMSC, FFUC_PRODUCT_UCX, 5),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UCX_IF5_SMSC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_RME, FFUC_PRODUCT_UCX, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UCX_ISO_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_RME, FFUC_PRODUCT_UCX, 5),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_UCX_IF5_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_SMSC, FFUC_PRODUCT_BABYFACE_CLASSIC, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_BABYFACE_CLASSIC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FFUC_VENDOR_RME, FFUC_PRODUCT_BABYFACE_CLASSIC, 0),
	  .driver_info = (kernel_ulong_t)&ffuc_devices[FFUC_DEV_BABYFACE_CLASSIC] },
	{ }
};
MODULE_DEVICE_TABLE(usb, ffuc_ids);

/* -- sample format conversion --------------------------------------- */

static u32 ffuc_s32_to_f32(s32 v)
{
	u32 sign = 0, mag, mant;
	int e;

	if (!v)
		return 0;
	if (v < 0) {
		sign = 0x80000000;
		mag = -(u32)v;
	} else {
		mag = v;
	}
	e = fls(mag) - 1;
	if (e > 23)
		mant = mag >> (e - 23);
	else
		mant = mag << (23 - e);
	return sign | ((u32)(e - 31 + 127) << 23) | (mant & 0x7fffff);
}

static s32 ffuc_f32_to_s32(u32 f)
{
	u32 exp = (f >> 23) & 0xff;
	u32 mant = (f & 0x7fffff) | 0x800000;
	bool neg = f >> 31;
	int shift;
	u32 mag;

	if (exp == 0xff && (f & 0x7fffff))
		return 0;
	shift = (int)exp - 119;
	if (shift >= 8)
		return neg ? S32_MIN : S32_MAX;
	if (shift >= 0)
		mag = mant << shift;
	else if (shift > -24)
		mag = mant >> -shift;
	else
		return 0;
	return neg ? -(s32)mag : (s32)mag;
}

/* -- streaming engine ------------------------------------------------ */

/* Ring-pressure attribution: the first event says new; later events are
 * rate-limited so a run stays readable in the journal. */
static void ffuc_ring_event(struct ffuc *chip, struct snd_pcm_substream *subs,
			    atomic_t *counter, const char *what,
			    unsigned long appl, unsigned long own)
{
	atomic_inc(counter);
	dev_dbg_ratelimited(&chip->dev->dev,
			    "%s: %s #%d appl %lu hw %lu (buffer %lu)\n",
			    subs->stream ? "capture" : "playback", what,
			    atomic_read(counter), appl, own,
			    subs->runtime->buffer_size);
}

/* Ring-fill model trace: sample the ring at a playback URB completion,
 * before the copy advances the read position.  `read_pos` is the ring
 * position this completion reads; `status_hw` is the published pointer,
 * which lags the driver position until the next period signal.  The
 * caller holds the playback stream lock.  Diagnostic only.
 *
 * The pointer difference is signed: when the application is behind the
 * read position (a dry ring that kept sending), the modular span would
 * read near-boundary and mask every later event.  A negative difference
 * clamps the fill to zero and sets FFUC_RING_F_BEHIND. */
static void ffuc_ring_trace(struct ffuc *chip, struct snd_pcm_substream *subs,
			    unsigned long read_pos, unsigned long appl,
			    unsigned long status_hw, unsigned int total)
{
	struct snd_pcm_runtime *rt = subs->runtime;
	unsigned long span = (appl + rt->boundary - read_pos) % rt->boundary;
	unsigned long pspan = (appl + rt->boundary - status_hw) % rt->boundary;
	unsigned long filled = 0, pub = 0;
	u64 now = ktime_get_ns();
	u32 flags = 0;
	struct ffuc_ring_rec *r;

	/* The true difference is within a few buffers of zero, far below
	 * boundary/2: a span above boundary/2 is a negative difference. */
	if (span <= rt->boundary / 2)
		filled = span;
	else
		flags |= FFUC_RING_F_BEHIND;
	if (pspan <= rt->boundary / 2)
		pub = pspan;
	if (total > filled)
		flags |= FFUC_RING_F_STARVED;
	if (total > pub)
		flags |= FFUC_RING_F_UNDERRUN;
	if (chip->ring_last_appl_ts) {
		if (appl == chip->ring_last_appl) {
			/* A write is overdue when a full period passes
			 * without an appl_ptr advance.  At period > URB
			 * the host writes once per period, so quiet URB
			 * cycles are normal; count the episode once, at
			 * the completion that crosses the period mark. */
			u64 period_ns = (u64)rt->period_size * NSEC_PER_SEC /
					rt->rate;
			u64 gap = now - chip->ring_last_appl_ts;

			if (gap >= period_ns &&
			    chip->ring_prev_ts - chip->ring_last_appl_ts <
			    period_ns) {
				flags |= FFUC_RING_F_MISSED;
				atomic_inc(&chip->ring_missed);
			}
		} else {
			u64 gap = now - chip->ring_last_appl_ts;

			if (gap > chip->ring_max_stall_ns)
				chip->ring_max_stall_ns = gap;
		}
	}
	if (appl != chip->ring_last_appl) {
		chip->ring_last_appl = appl;
		chip->ring_last_appl_ts = now;
	}
	chip->ring_prev_ts = now;
	if (filled < chip->ring_min_filled)
		chip->ring_min_filled = filled;
	if (flags & FFUC_RING_F_STARVED)
		atomic_inc(&chip->ring_starved);

	r = &chip->ring_trace[chip->ring_head % FFUC_RING_TRACE];
	r->ktime_ns = now;
	r->appl = appl;
	r->hw = read_pos;
	r->status_hw = (u32)status_hw;
	r->filled = (u32)filled;
	r->total = total;
	r->flags = flags;
	chip->ring_head++;
}

/* The core's shared status pointer diverged from this stream's driver
 * pointer right after a period-elapsed refresh.  Count every occurrence;
 * the first one dmesg-logs the two values. */
static void ffuc_ptr_mismatch(struct ffuc *chip,
			      struct snd_pcm_substream *subs,
			      const struct ffuc_stream *s)
{
	atomic_inc(&chip->hw_ptr_mismatch);
	if (!(chip->hw_ptr_logged++)) {
		struct snd_pcm_runtime *rt = subs->runtime;

		dev_warn_ratelimited(&chip->dev->dev,
				     "%s status hw_ptr %lu driver hw_ptr %lu (buffer %lu period %lu): core pointer diverged\n",
				     subs->stream ? "capture" : "playback",
				     rt->status->hw_ptr, s->hw_ptr,
				     rt->buffer_size, rt->period_size);
	} else {
		dev_dbg_ratelimited(&chip->dev->dev,
				    "%s mismatch #%u status %lu driver %lu\n",
				    subs->stream ? "capture" : "playback",
				    chip->hw_ptr_logged,
				    subs->runtime->status->hw_ptr, s->hw_ptr);
	}
}

/*
 * At 4x the interrupt transport has two capture endpoints: endpoint 0
 * carries words 0..words-1, endpoint 1 carries the next slice.  Each
 * endpoint must only zero channels past the *total* active word count;
 * clearing past its own slice would wipe the other endpoint's channels
 * whenever endpoint 0 completes after endpoint 1.
 */
static void ffuc_capture_copy(struct snd_pcm_runtime *rt, snd_pcm_uframes_t pos,
			      const __le32 *src, unsigned int frames,
			      unsigned int nch, unsigned int coff,
			      unsigned int fstride, unsigned int cstride,
			      unsigned int total)
{
	bool is_float = rt->format == SNDRV_PCM_FORMAT_FLOAT_LE;
	unsigned int chans = rt->channels;
	unsigned int p = pos % rt->buffer_size;
	unsigned int f, c;

	for (f = 0; f < frames; f++, src += fstride) {
		u32 *dst = (u32 *)rt->dma_area + p * chans;

		for (c = 0; c < nch && coff + c < chans; c++) {
			u32 v = le32_to_cpu(src[c * cstride]);

			dst[coff + c] = is_float ? v : (u32)ffuc_f32_to_s32(v);
		}
		/* 2x/4x packets carry only 14/12 words; keep the logical
		 * 18-channel ALSA buffer silent on the unused ADAT slots.
		 * Only clear channels that no endpoint fills. */
		for (; c + coff < chans; c++)
			if (coff + c >= total)
				dst[coff + c] = 0;
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static void ffuc_capture_copy_int32(struct snd_pcm_runtime *rt,
				    snd_pcm_uframes_t pos,
				    const __le32 *src, unsigned int frames,
				    unsigned int nch, unsigned int coff,
				    unsigned int fstride, unsigned int cstride,
				    unsigned int total)
{
	bool is_float = rt->format == SNDRV_PCM_FORMAT_FLOAT_LE;
	unsigned int chans = rt->channels;
	unsigned int p = pos % rt->buffer_size;
	unsigned int f, c;

	for (f = 0; f < frames; f++, src += fstride) {
		u32 *dst = (u32 *)rt->dma_area + p * chans;

		for (c = 0; c < nch && coff + c < chans; c++) {
			u32 v = le32_to_cpu(src[c * cstride]);

			dst[coff + c] = is_float ? ffuc_s32_to_f32((s32)v) : v;
		}
		for (; c + coff < chans; c++)
			if (coff + c >= total)
				dst[coff + c] = 0;
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static void ffuc_playback_copy(struct snd_pcm_runtime *rt, snd_pcm_uframes_t pos,
			       __le32 *dst, unsigned int frames,
			       unsigned int nch, unsigned int coff,
			       unsigned int fstride)
{
	bool is_float = rt->format == SNDRV_PCM_FORMAT_FLOAT_LE;
	unsigned int chans = rt->channels;
	unsigned int p = pos % rt->buffer_size;
	unsigned int f, c;

	for (f = 0; f < frames; f++, dst += fstride) {
		const u32 *src = (const u32 *)rt->dma_area + p * chans;

		for (c = 0; c < nch; c++) {
			u32 v = 0;

			if (coff + c < chans)
				v = is_float ? src[coff + c] :
				    ffuc_s32_to_f32((s32)src[coff + c]);
			dst[c] = cpu_to_le32(v);
		}
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static void ffuc_playback_copy_int32(struct snd_pcm_runtime *rt,
				     snd_pcm_uframes_t pos, __le32 *dst,
				     unsigned int frames, unsigned int nch,
				     unsigned int coff, unsigned int fstride)
{
	bool is_float = rt->format == SNDRV_PCM_FORMAT_FLOAT_LE;
	unsigned int chans = rt->channels;
	unsigned int p = pos % rt->buffer_size;
	unsigned int f, c;

	for (f = 0; f < frames; f++, dst += fstride) {
		const u32 *src = (const u32 *)rt->dma_area + p * chans;

		for (c = 0; c < nch; c++) {
			u32 v = 0;

			if (coff + c < chans)
				v = is_float ?
				    (u32)ffuc_f32_to_s32(src[coff + c]) :
				    src[coff + c];
			dst[c] = cpu_to_le32(v);
		}
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static bool ffuc_stream_advance(struct ffuc *chip, struct ffuc_stream *s,
				struct snd_pcm_runtime *rt, unsigned int idx,
				unsigned int frames)
{
	snd_pcm_uframes_t hw;
	int i;

	s->pos[idx] += frames;
	hw = s->pos[0];
	for (i = 1; i < chip->mode->neps; i++)
		hw = min(hw, s->pos[i]);
	s->hw_ptr = hw;
	if (hw / rt->period_size == s->period)
		return false;
	s->period = hw / rt->period_size;
	return true;
}

/* A direction may be signalled only when its period index has moved
 * since its last signal: a signal is the announcement of one completed
 * period. */
static bool ffuc_pending(struct ffuc_stream *s)
{
	return s->running && s->period != s->signalled;
}

static void ffuc_signal_period(struct ffuc *chip, struct ffuc_stream *s)
{
	unsigned long flags;

	if (!s->substream)
		return;
	snd_pcm_stream_lock_irqsave(s->substream, flags);
	if (!ffuc_pending(s)) {
		/* A gate violation is only countable while the stream
		 * runs; a stop racing the caller is not a violation. */
		if (s->running)
			atomic_inc(&chip->delta_zero);
	} else {
		s->signalled = s->period;
		snd_pcm_period_elapsed_under_stream_lock(s->substream);
		/* The core refreshes the shared status pointer from
		 * .pointer inside the period-elapsed call.  It must
		 * re-read the same value this driver maintains; a
		 * difference means the core moved hw_base by itself
		 * (the double-interrupt jiffies heuristic). */
		if (s->substream->runtime->status->hw_ptr !=
		    s->hw_ptr % s->substream->runtime->boundary)
			ffuc_ptr_mismatch(chip, s->substream, s);
	}
	snd_pcm_stream_unlock_irqrestore(s->substream, flags);
}

/*
 * Signal a period.  The graph is woken when both running directions have
 * completed a period: at whichever boundary is later, derived every cycle,
 * so the follower's pointer is never stale when the reader wakes and no
 * direction is ever signalled for zero completed periods.  When one
 * direction is behind, the wake is withheld; a direction whose wake has
 * been withheld twice signals alone, so a stalled or missing stream cannot
 * hold the other hostage.  Single-direction operation signals that
 * direction at its own boundary.
 */
static void ffuc_period_elapsed(struct ffuc *chip, int dir)
{
	struct ffuc_stream *p = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	struct ffuc_stream *c = &chip->streams[SNDRV_PCM_STREAM_CAPTURE];
	unsigned long flags;

	/* The two directions' completions can run concurrently on
	 * different CPUs; the gate decision and its deferral counters
	 * form one serialized section. Lock order: gate_lock, then the
	 * stream lock inside ffuc_signal_period.
	 */
	spin_lock_irqsave(&chip->gate_lock, flags);
	if (!p->running || !c->running) {
		ffuc_signal_period(chip, p->running ? p : c);
		spin_unlock_irqrestore(&chip->gate_lock, flags);
		return;
	}
	{
		u32 mp = max((u32)(p->period - p->signalled),
			     (u32)(c->period - c->signalled));

		if (mp > chip->max_pending)
			chip->max_pending = mp;
	}
	if (ffuc_pending(p) && ffuc_pending(c)) {
		ffuc_signal_period(chip, p);
		ffuc_signal_period(chip, c);
		p->deferrals = 0;
		c->deferrals = 0;
		spin_unlock_irqrestore(&chip->gate_lock, flags);
		return;
	}
	atomic_inc(&chip->signal_deferred);
	/* One direction is behind: withhold the wake until the later
	 * boundary, but never longer than two completions.  Counting
	 * deferrals instead of periods bounds the wait to about one URB at
	 * every period size.
	 */
	if (ffuc_pending(p) && ++p->deferrals >= 2) {
		atomic_inc(&chip->signal_overdue);
		ffuc_signal_period(chip, p);
		p->deferrals = 0;
	}
	if (ffuc_pending(c) && ++c->deferrals >= 2) {
		atomic_inc(&chip->signal_overdue);
		ffuc_signal_period(chip, c);
		c->deferrals = 0;
	}
	spin_unlock_irqrestore(&chip->gate_lock, flags);
}

/* Interrupt transport: one URB of interleaved frames. */
static void ffuc_capture_urb(struct ffuc_ep *ep, struct urb *urb)
{
	struct ffuc *chip = ep->chip;
	struct ffuc_stream *s = &chip->streams[SNDRV_PCM_STREAM_CAPTURE];
	struct snd_pcm_substream *subs = s->substream;
	const unsigned int words = chip->mode->words;
	const unsigned int total = chip->mode->neps * words;
	unsigned int frames = urb->actual_length / (words * FFUC_SAMPLE_BYTES);
	unsigned long flags;
	bool elapsed = false;
	bool bad = false;

	/* A remainder shorter than one frame carries no complete channel
	 * set; copying it would shift the channel order for the rest of
	 * the stream.  An overlong payload cannot exceed the buffer.  The
	 * floor is copied and the remainder discarded, so the ring and
	 * the wire stay frame-aligned; the anomaly is counted, like the
	 * playback side. */
	if (urb->actual_length % (words * FFUC_SAMPLE_BYTES))
		bad = true;
	if (urb->actual_length > urb->transfer_buffer_length ||
	    frames > chip->urb_frames) {
		frames = chip->urb_frames;
		bad = true;
	}
	if (bad)
		atomic_inc(&chip->in_bad_length);

	if (!frames)
		return;

	snd_pcm_stream_lock_irqsave(subs, flags);
	if (s->running) {
		struct snd_pcm_runtime *rt = subs->runtime;
		/* A completion that overwrites unread frames means the
		 * reader lagged the device by more than the ring minus
		 * this payload.  Unread bytes are the ring span [appl_ptr,
		 * hw_ptr); a write at [hw_ptr, hw_ptr + frames) collides
		 * with unread data when it passes appl_ptr + buffer_size.
		 * The write position is this endpoint's own ring position,
		 * not the published status hw_ptr (which lags until the next
		 * period signal).  ALSA compares pointers modulo
		 * runtime->boundary, so the span length is taken modularly,
		 * not by plain subtraction. */
		unsigned long appl = READ_ONCE(rt->control->appl_ptr);
		unsigned long own = s->pos[ep->index] % rt->boundary;
		unsigned long unread = (own + rt->boundary - appl) %
				       rt->boundary;

		if (unread + frames > rt->buffer_size)
			ffuc_ring_event(chip, subs, &chip->in_overruns,
					"capture overwrite", appl, own);
		if (chip->s32_samples)
			ffuc_capture_copy_int32(subs->runtime, s->pos[ep->index],
						urb->transfer_buffer, frames, words,
						ep->index * words, words, 1, total);
		else
			ffuc_capture_copy(subs->runtime, s->pos[ep->index],
					  urb->transfer_buffer, frames, words,
					  ep->index * words, words, 1, total);
		elapsed = ffuc_stream_advance(chip, s, subs->runtime,
					      ep->index, frames);
	}
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	if (elapsed)
		ffuc_period_elapsed(chip, SNDRV_PCM_STREAM_CAPTURE);
}

static void ffuc_playback_urb(struct ffuc_ep *ep, struct urb *urb)
{
	struct ffuc *chip = ep->chip;
	struct ffuc_stream *s = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	struct snd_pcm_substream *subs = s->substream;
	const unsigned int words = ffuc_mode_out_words(chip->mode);
	unsigned int frames = urb->actual_length / (words * FFUC_SAMPLE_BYTES);
	unsigned long flags;
	bool elapsed = false;
	bool bad = false;
	bool starved = false;
	bool stop = false;

	/* The engine submitted this OUT URB with urb_frames frames, so the
	 * nominal case equals it.  Copy exactly what left the wire, and
	 * never more than one URB holds: a remainder shorter than one
	 * frame carries no complete channel set and must not shift the
	 * ring; an overlong payload cannot exceed the buffer.  Both
	 * anomalies are counted, like the capture side. */
	if (urb->actual_length % (words * FFUC_SAMPLE_BYTES))
		bad = true;
	if (frames > chip->urb_frames) {
		frames = chip->urb_frames;
		bad = true;
	}
	if (bad)
		atomic_inc(&chip->out_bad_length);
	if (!frames)
		return;

	snd_pcm_stream_lock_irqsave(subs, flags);
	if (s->running) {
		struct snd_pcm_runtime *rt = subs->runtime;
		/* Starvation is measured against the ring position this
		 * endpoint reads, not the published status hw_ptr (which
		 * lags until the next period signal).  Unwritten ring
		 * content is never valid playback: a starved URB carries
		 * silence.  One starved URB is a grace (a single late write
		 * becomes a one-URB gap); two consecutive starved URBs stop
		 * the stream as an XRUN below. */
		unsigned long appl = READ_ONCE(rt->control->appl_ptr);
		unsigned long own = s->pos[ep->index] % rt->boundary;
		unsigned long filled = (appl + rt->boundary - own) %
				       rt->boundary;

		if (frames > filled) {
			starved = true;
			if (!s->out_starve_streak)
				ffuc_ring_event(chip, subs, &chip->out_underruns,
						"playback underrun", appl, own);
			s->out_starve_streak++;
			stop = s->out_starve_streak >= 2 && !s->out_start_urbs;
		} else {
			s->out_starve_streak = 0;
		}
		if (s->out_start_urbs)
			s->out_start_urbs--;
		ffuc_ring_trace(chip, subs, s->pos[ep->index], appl, own,
				frames);
		if (starved) {
			memset(urb->transfer_buffer, 0,
			       urb->transfer_buffer_length);
		} else if (chip->s32_samples) {
			ffuc_playback_copy_int32(subs->runtime, s->pos[ep->index],
						 urb->transfer_buffer, frames,
						 words, ep->index * words,
						 words);
			elapsed = ffuc_stream_advance(chip, s, subs->runtime,
						      ep->index, frames);
		} else {
			ffuc_playback_copy(subs->runtime, s->pos[ep->index],
					   urb->transfer_buffer, frames, words,
					   ep->index * words, words);
			elapsed = ffuc_stream_advance(chip, s, subs->runtime,
						      ep->index, frames);
		}
	} else {
		memset(urb->transfer_buffer, 0, urb->transfer_buffer_length);
	}
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	if (stop)
		snd_pcm_stop_xrun(subs);
	if (elapsed)
		ffuc_period_elapsed(chip, SNDRV_PCM_STREAM_PLAYBACK);
}

/* ISO implicit feedback: remember every IN packet's position and frame
 * count by packet number. */
static void ffuc_fb_put(struct ffuc *chip, unsigned int pos, unsigned int frames,
			unsigned int phase, bool seed)
{
	u32 e;

	/* Header layout per rate band: frame count bits 0-4, position bits
	 * 5..(4+width), phase the remaining upper bits (see ffuc_iso_layouts). */
	e = ((phase & ffuc_iso_phase_mask(chip)) << ffuc_iso_phase_shift(chip)) |
	    ((pos & ffuc_iso_pos_mask(chip)) << 5) | (frames & 0x1f);
	chip->fb[chip->in_pkts++ & (FFUC_FB_SIZE - 1)] = e;
	chip->in_end = (pos + frames) % ffuc_iso_wrap(chip);
	/* The unwrapped position advances exactly like the wrap-domain
	 * in_end, so the two stay congruent across a 8192 wrap.  Only a
	 * real header packet seeds the unwrapped origin, at the field's
	 * current value: the estimated path passes the wrap-domain in_end
	 * as its position, and in_end is zero at a stream start while the
	 * device's field continues from its own value.  Seeding from the
	 * estimate would offset the two domains by the device's absolute
	 * position. */
	if (seed && !chip->in_total_valid) {
		chip->in_total = pos;
		chip->in_field = pos;
		chip->in_total_valid = true;
	}
	if (chip->in_total_valid) {
		chip->in_total += frames;
		if (seed) {
			/* Field model: the device's own position, jumps
			 * included (the lead words are compared against it).
			 * A non-zero jump against the frame-count advance is
			 * the field's occasional resync; the count model
			 * (in_total) stays continuous and drives rate_meas. */
			s32 jump = (s32)((pos + ffuc_iso_wrap(chip) -
					  (u32)(chip->in_field % ffuc_iso_wrap(chip))) %
					 ffuc_iso_wrap(chip));
			if (jump > (s32)(ffuc_iso_wrap(chip) / 2))
				jump -= ffuc_iso_wrap(chip);
			chip->in_field += jump + frames;
		}
	}
}

static void ffuc_fb_put_nominal(struct ffuc *chip)
{
	chip->fb_acc += chip->rate;
	ffuc_fb_put(chip, chip->in_end, chip->fb_acc / 8000, 0, false);
	chip->fb_acc %= 8000;
}

static bool ffuc_fb_get(struct ffuc *chip, u64 k, unsigned int *pos,
			unsigned int *frames, unsigned int *phase)
{
	u64 src = k - chip->fb_delay;
	u32 e;

	if (k < chip->fb_delay || src >= chip->in_pkts ||
	    chip->in_pkts - src > FFUC_FB_SIZE)
		return false;
	e = chip->fb[src & (FFUC_FB_SIZE - 1)];
	*pos = (e >> 5) & ffuc_iso_pos_mask(chip);
	*phase = (e >> ffuc_iso_phase_shift(chip)) & ffuc_iso_phase_mask(chip);
	*frames = min(e & 0x1f, chip->mode->max_frames);
	return true;
}

/*
 * The lead anchor: the value the fb-ring seed produced, computed from
 * the device timestamp instead of a ring entry.
 *
 * The old seed read the position the device reported for the fb-ring
 * slot the fill consumes (ring slot out_pkts − fb_delay) and added
 * out_lead.  The device timestamp carries the same timeline as the
 * unwrapped in_total: the position at that slot is in_total rewound by
 * the frames the device captured in the packets between the slot and
 * the fill moment — (in_pkts − out_pkts + fb_delay) packets at the
 * nominal rate, Q32.  The call site gates the seed on ts_pos_valid: a
 * real capture header must have arrived.  During the priming window
 * the rewind can reach before the first header; the position counter
 * free-runs at the committed rate, so the extrapolated position is on
 * the same timeline.  The fraction stays the last header's fraction
 * (sub-frame error over the rewind).
 *
 * Exact to a frame today (the interpolation over a few ms is
 * sub-frame); sub-sample once the phase convention is captured
 * (DRIVER-DESIGN §4.3).  Everything is mod the wrap: in_total is
 * congruent with the wrap-domain in_end, and the lead-word writes
 * reduce mod the wrap.
 */
static u32 ffuc_lead_seed(struct ffuc *chip)
{
	u64 wrap = ffuc_iso_wrap(chip);
	u64 step_q32 = div64_u64((u64)chip->rate << 32, 8000);
	s64 span_pkts, pos_q32;

	/* Packets between the fb slot the seed represents and the fill
	 * moment.  The fill consumes fb slot (out_pkts − fb_delay); the
	 * device position at that slot is in_total rewound. */
	span_pkts = (s64)chip->in_pkts - (s64)chip->out_pkts
		  + (s64)chip->fb_delay;
	pos_q32 = (s64)(((chip->ts_pos % wrap) << 32) + chip->ts_frac_q32)
		- span_pkts * (s64)step_q32;
	chip->ts_seed_span = span_pkts;
	chip->ts_seed_pos_q32 = (u64)pos_q32;
	chip->ts_seed_in = chip->in_pkts;
	chip->ts_seed_out = chip->out_pkts;
	chip->ts_seed_in_end = chip->in_end;

	/* Floor at Q32 (arithmetic shift); out_lead then the wrap mask.
	 * The mask is a power of two, so it is the true residue. */
	chip->ts_seed_counter =
		(u32)(((pos_q32 >> 32) + (s64)chip->out_lead) & (s64)(wrap - 1));
	return chip->ts_seed_counter;
}

/*
 * Cadence check: the mean frames per packet times 8000 is the device
 * clock the packets reveal.  A disagreement with chip->rate means the
 * rate write did not take effect (or the device was reprogrammed).
 * This whole-run mean drives the periodic guard; the stop verdict uses
 * the converged mean (ffuc-cadence.h) instead.
 */
static unsigned int ffuc_iso_cadence(struct ffuc *chip, int *mismatch)
{
	u64 frames = 0, pkts = 0;
	unsigned int i, mean;

	for (i = 0; i < ARRAY_SIZE(chip->iso_frames_hist); i++) {
		frames += (u64)i * chip->iso_frames_hist[i];
		pkts += chip->iso_frames_hist[i];
	}
	if (!pkts)
		return 0;
	mean = (unsigned int)((frames * 8000) / pkts);
	if (mismatch && ffuc_cadence_mismatch(chip->rate, mean))
		*mismatch = 1;
	return mean;
}

static void ffuc_iso_capture_urb(struct ffuc_ep *ep, struct urb *urb)
{
	struct ffuc *chip = ep->chip;
	struct ffuc_stream *s = &chip->streams[SNDRV_PCM_STREAM_CAPTURE];
	struct snd_pcm_substream *subs = s->substream;
	const struct ffuc_mode *m = chip->mode;
	const unsigned int nch = m->words;
	unsigned int frames[FFUC_MAX_ISO_PACKETS];
	unsigned long flags;
	bool elapsed = false;
	int i;
	u64 f_base;

	spin_lock_irqsave(&chip->fb_lock, flags);

	/* Start-frame anchor: sof_ns(f) = ts_anchor_ns + f * 125000.  For
	 * high-speed, urb->start_frame advances one unit per ISO packet
	 * (125 us microframes; the driver's own continuity math,
	 * next_start = start_frame + number_of_packets, is in the same
	 * units).  The completion lands shortly after the URB's last
	 * packet window, so the raw anchor is the completion latency.  An
	 * exponential average (1/8) keeps it jitter-free while tracking
	 * the slow host-vs-controller clock offset; a discontinuity
	 * (engine restart, controller hiccup) resyncs it. */
	{
		u64 f_end = chip->ts_frames + urb->number_of_packets;
		u64 a_raw = ktime_get_ns() - f_end * 125000ull;
		s64 d;

		if (!chip->ts_anchor_valid) {
			chip->ts_anchor_ns = a_raw;
			chip->ts_anchor_valid = true;
		} else {
			d = (s64)(a_raw - chip->ts_anchor_ns);
			if (d > 2000000 || d < -2000000) {
				atomic_inc(&chip->in_anchor_resync);
				chip->ts_anchor_ns = a_raw;
			} else {
				chip->ts_anchor_ns += d / 8;
			}
		}
	}
	f_base = chip->ts_frames;
	for (i = 0; i < urb->number_of_packets; i++) {
		const struct usb_iso_packet_descriptor *d =
			&urb->iso_frame_desc[i];
		const u8 *pkt = (const u8 *)urb->transfer_buffer + d->offset;
		u32 hdr;

		if (d->status) {
			atomic_inc(&chip->in_status_err);
			frames[i] = 0;
			ffuc_fb_put_nominal(chip);
			continue;
		}
		if (d->actual_length < FFUC_ISO_IN_HEADER) {
			atomic_inc(&chip->in_short);
			frames[i] = 0;
			ffuc_fb_put_nominal(chip);
			continue;
		}
		if ((d->actual_length - FFUC_ISO_IN_HEADER) %
		    (nch * FFUC_SAMPLE_BYTES)) {
			atomic_inc(&chip->in_misaligned);
			frames[i] = 0;
			ffuc_fb_put_nominal(chip);
			continue;
		}
		frames[i] = min((d->actual_length - FFUC_ISO_IN_HEADER) /
				(nch * FFUC_SAMPLE_BYTES), m->max_frames);
		if (!frames[i])
			atomic_inc(&chip->in_hdr_only);
		if (frames[i] < ARRAY_SIZE(chip->iso_frames_hist)) {
			chip->iso_frames_hist[frames[i]]++;
		} else {
			atomic_inc(&chip->in_frames_overflow);
		}
		/* Periodic cadence guard: the packets reveal the device
		 * clock; a mismatch means the rate write did not take
		 * effect or the device was reprogrammed externally. */
		if ((chip->in_pkts % 8000) == 0) {
			int mm = 0;

			if (atomic_read(&chip->in_frames_overflow))
				goto cadence_skip;
			chip->iso_cadence = ffuc_iso_cadence(chip, &mm);
			if (mm) {
				if (++chip->rate_mismatch_windows >= 2) {
					atomic_inc(&chip->rate_mismatch);
					dev_warn_ratelimited(&chip->dev->dev,
							     "ISO cadence %u Hz vs rate %u Hz\n",
							     chip->iso_cadence,
							     chip->rate);
					chip->rate_mismatch_windows = 0;
				}
			} else {
				chip->rate_mismatch_windows = 0;
			}
cadence_skip:
			;
		}
		hdr = le32_to_cpup((const __le32 *)pkt);
		/* Header layout per rate band (ffuc_iso_layouts): frame count
		 * bits 0-4, position bits 5..(4+width) wrapping at the band
		 * modulus, phase the remaining upper bits.  The wrap is a power
		 * of two, so the field mask and the modulo reduction agree.  A
		 * header-only packet carries the current position with zero
		 * frames; it must advance the feedback position, not reset it. */
		{
			unsigned int pos = (hdr >> 5) & ffuc_iso_pos_mask(chip);
			unsigned int hframes = hdr & 0x1f;
			unsigned int phase = (hdr >> ffuc_iso_phase_shift(chip)) &
					     ffuc_iso_phase_mask(chip);
			unsigned int wrap = ffuc_iso_wrap(chip);

			pos %= wrap;
			if (hframes != frames[i])
				atomic_inc(&chip->in_frames_mismatch);
			if (chip->in_pkts &&
			    pos != chip->in_end % wrap) {
				/* The device emits a start ramp at every engine
				 * start: header-only and sub-cadence packets at
				 * position 0, then the position field joins the
				 * free-running timeline.  The first discontinuity
				 * of a run is that start resync; it re-seeds the
				 * expectation (fb_put below) and is not an error.
				 * Later discontinuities count. */
				if (chip->in_start_resync)
					atomic_inc(&chip->in_pos_gap);
				else
					chip->in_start_resync = true;
			}

			/* Device timestamp: this header's (position, phase)
			 * describe one boundary, the packet's own.  The
			 * phase law (gate 3 captures): the value wraps at
			 * tps, advances one microframe per packet mod
			 * tps, and the exact position is pos + phase/tps
			 * (the device is ahead by the owed fraction; the
			 * add pays it at the wrap).  The whole-frame
			 * position is the unwrapped in_total before this
			 * packet's frames, taken after the put advanced
			 * it. */
			ffuc_fb_put(chip, pos, frames[i], phase, true);
			{
				u64 pos_k = chip->in_field - frames[i];
				u64 cnt_k = chip->in_total - frames[i];
				u64 ts_ns = chip->ts_anchor_ns +
					(f_base + i) * 125000ull;
				/* phase/tps as Q32 frames:
				 * phase * rate * 2^32 / 60e6 (60 MHz tick). */
				u64 frac = mul_u64_u64_div_u64(
					phase, (u64)chip->rate << 32,
					60000000ull);
				u64 exact_cnt = (cnt_k << 32) + frac;
				u64 dq, dn;

				(void)pos_k;

				chip->ts_ns = ts_ns;
				chip->ts_pos = pos_k;
				chip->ts_phase = phase;
				chip->ts_frac_q32 = frac;
				chip->ts_pos_valid = true;

				/* rate_meas over a one-second window, from the
				 * exact (sub-sample) position. */
				if (ts_ns - chip->ts_win_ns >= 1000000000ull) {
					if (chip->ts_win_ns) {
						dq = exact_cnt - chip->ts_win_pos;
						dn = ts_ns - chip->ts_win_ns;
						chip->ts_rate_meas = div64_u64(
							(dq >> 32) * 1000000000ull +
							(((dq & 0xffffffffull) *
							 1000000000ull) >> 32),
							dn);
					}
					chip->ts_win_ns = ts_ns;
					chip->ts_win_pos = exact_cnt;
				}
			}
		}
	}
	chip->ts_frames += urb->number_of_packets;
	{
		unsigned int urb_frames = 0;

		for (i = 0; i < urb->number_of_packets; i++)
			urb_frames += frames[i];
		ffuc_cadence_window(&chip->cad, chip->rate, urb_frames,
				    urb->number_of_packets);
	}
	spin_unlock_irqrestore(&chip->fb_lock, flags);

	if (!subs) {
		/* Feedback-only capture: no PCM attached. */
		atomic_inc(&chip->fb_only_urbs);
		return;
	}
	if (!s->running)
		atomic_inc(&chip->fb_only_urbs);
	snd_pcm_stream_lock_irqsave(subs, flags);
	if (s->running) {
		struct snd_pcm_runtime *rt = subs->runtime;
		unsigned int total = 0;
		unsigned long appl, own, unread;

		for (i = 0; i < urb->number_of_packets; i++)
			total += frames[i];
		/* A write that passes appl_ptr + buffer_size overwrites
		 * frames the reader has not consumed.  The write position is
		 * the driver's own ring position, not the published status
		 * hw_ptr (which lags until the next period signal). */
		appl = READ_ONCE(rt->control->appl_ptr);
		own = s->hw_ptr % rt->boundary;
		unread = (own + rt->boundary - appl) % rt->boundary;
		if (unread + total > rt->buffer_size)
			ffuc_ring_event(chip, subs, &chip->in_overruns,
					"capture overwrite", appl, own);
		for (i = 0; i < urb->number_of_packets; i++) {
			const u8 *pkt = (const u8 *)urb->transfer_buffer +
					urb->iso_frame_desc[i].offset;

			if (!frames[i])
				continue;
			if (chip->s32_samples)
				ffuc_capture_copy_int32(subs->runtime, s->pos[0],
					  (const __le32 *)(pkt + FFUC_ISO_IN_HEADER),
					  frames[i], nch, 0, 1, frames[i], nch);
			else
				ffuc_capture_copy(subs->runtime, s->pos[0],
					  (const __le32 *)(pkt + FFUC_ISO_IN_HEADER),
					  frames[i], nch, 0, 1, frames[i], nch);
			elapsed |= ffuc_stream_advance(chip, s, subs->runtime, 0,
						       frames[i]);
		}
	}
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	if (elapsed)
		ffuc_period_elapsed(chip, SNDRV_PCM_STREAM_CAPTURE);
}

/*
 * ISO OUT packet:
 *   UCX / Babyface-classic: frames interleaved, each frame = lead word +
 *        nch float32 samples.  Every lead word is the stream position that
 *        frame plays at (mod 8192), keeping each frame self-describing.
 *   UC:  frames interleaved, each frame = lead word + nch float32 samples;
 *        only the first lead word is used by the device, the rest are
 *        padding.
 */
static void ffuc_iso_playback_fill(struct ffuc_ep *ep, struct urb *urb)
{
	struct ffuc *chip = ep->chip;
	struct ffuc_stream *s = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	struct snd_pcm_substream *subs = s->substream;
	const struct ffuc_mode *m = chip->mode;
	const unsigned int nch = ffuc_mode_out_words(m);
	const unsigned int fwords = nch + 1;
	unsigned int frames[FFUC_MAX_ISO_PACKETS], pos[FFUC_MAX_ISO_PACKETS];
	unsigned int phase[FFUC_MAX_ISO_PACKETS];
	unsigned int off = 0;
	unsigned long flags;
	bool elapsed = false;
	bool starved = false;
	bool stop = false;
	int i;

	/* Per-packet delivery check from this URB's previous submission in
	 * the same engine run.  Engine setup clears the descriptors of the
	 * reused URBs, so the first fill of a run judges nothing. */
	for (i = 0; i < urb->number_of_packets; i++) {
		const struct usb_iso_packet_descriptor *d =
			&urb->iso_frame_desc[i];

		if (d->length && d->actual_length != d->length) {
			if (d->status)
				atomic_inc(&chip->out_pkt_status);
			else
				atomic_inc(&chip->out_pkt_len_err);
		}
	}

	spin_lock_irqsave(&chip->fb_lock, flags);
	for (i = 0; i < urb->number_of_packets; i++) {
		u64 k = chip->out_pkts++;
		u64 src = k - chip->fb_delay;

		phase[i] = 0;
		if (!ffuc_fb_get(chip, k, &pos[i], &frames[i], &phase[i])) {
			if (k < chip->fb_delay) {
				atomic_inc(&chip->out_fb_priming);
				/* Priming carries nominal-cadence frames, never
				 * empty packets: the device output buffer must
				 * stay fed before feedback exists.  The
				 * fractional accumulator advances the lead at
				 * the exact nominal rate (rate/8000 per
				 * packet), so the handover to the mirrored
				 * counts carries no cadence step. */
				chip->out_prime_acc += chip->rate;
				frames[i] = min(chip->out_prime_acc / 8000,
						m->max_frames);
				chip->out_prime_acc %= 8000;
			} else {
				if (src >= chip->in_pkts)
					atomic_inc(&chip->out_fb_ahead);
				else
					atomic_inc(&chip->out_fb_stale);
				frames[i] = 0;
			}
		}
	}
	spin_unlock_irqrestore(&chip->fb_lock, flags);

	/* Capacity clamp before anything reads frames[]: a packet that does
	 * not fit is dropped whole, never truncated mid-frame, so the
	 * lead-word stride cannot desync. */
	{
		unsigned int cap_off = 0;

		for (i = 0; i < urb->number_of_packets; i++) {
			unsigned int len = frames[i] * fwords * FFUC_SAMPLE_BYTES;

			if (cap_off + len > ep->buf_size) {
				atomic_inc(&chip->out_capacity_err);
				frames[i] = 0;
				len = 0;
			}
			cap_off += len;
		}
	}

	snd_pcm_stream_lock_irqsave(subs, flags);
	if (s->running) {
		struct snd_pcm_runtime *rt = subs->runtime;
		unsigned int total = 0;
		unsigned long appl, own, filled;

		for (i = 0; i < urb->number_of_packets; i++)
			total += frames[i];
		/* The frames about to be copied are read from this stream's
		 * own ring position, so starvation is measured against it.
		 * The published status hw_ptr only refreshes on a period
		 * signal and lags the driver position by up to a period; it
		 * would mask a dry ring.  A starved URB carries silence, never
		 * unwritten ring content.  One starved URB is a grace (a
		 * single late write becomes a one-URB gap); two consecutive
		 * starved URBs stop the stream as an XRUN below. */
		appl = READ_ONCE(rt->control->appl_ptr);
		own = s->hw_ptr % rt->boundary;
		filled = (appl + rt->boundary - own) % rt->boundary;
		if (total > filled) {
			starved = true;
			if (!s->out_starve_streak)
				ffuc_ring_event(chip, subs, &chip->out_underruns,
						"playback underrun", appl, own);
			s->out_starve_streak++;
			stop = s->out_starve_streak >= 2 && !s->out_start_urbs;
		} else {
			s->out_starve_streak = 0;
		}
		if (s->out_start_urbs)
			s->out_start_urbs--;
		ffuc_ring_trace(chip, subs, s->hw_ptr, appl, own, total);
	}
	/* The seed needs a real device timestamp: ts_pos_valid means at
	 * least one capture header arrived.  Priming frames are nominal, so
	 * a non-zero frames[] no longer implies feedback; without the
	 * timestamp the rewind would extrapolate from a zero position. */
	if (s->running && !s->out_counter_valid && chip->ts_pos_valid) {
		for (i = 0; i < urb->number_of_packets; i++) {
			if (frames[i]) {
				s->out_counter = ffuc_lead_seed(chip);
				s->out_counter_valid = true;
				break;
			}
		}
	}
	/* The in-flight queue for .pointer's delay (R4): this URB's
	 * playback frames, added at submit and removed at completion. */
	{
		struct ffuc_urb *u = urb->context;
		unsigned int total = 0;

		for (i = 0; i < urb->number_of_packets; i++)
			total += frames[i];
		if (u)
			u->frames = total;
	}
	for (i = 0; i < urb->number_of_packets; i++) {
		__le32 *dst = (__le32 *)((u8 *)urb->transfer_buffer + off);
		unsigned int len = frames[i] * fwords * FFUC_SAMPLE_BYTES;
		unsigned int f;

		urb->iso_frame_desc[i].offset = off;
		urb->iso_frame_desc[i].length = len;
		if (s->running && frames[i] && !starved &&
		    s->out_counter_valid) {
			if (chip->s32_samples)
				ffuc_playback_copy_int32(subs->runtime, s->pos[0],
					   dst + 1, frames[i], nch, 0, fwords);
			else
				ffuc_playback_copy(subs->runtime, s->pos[0],
					   dst + 1, frames[i], nch, 0, fwords);
			elapsed |= ffuc_stream_advance(chip, s, subs->runtime, 0,
						       frames[i]);
		} else {
			/* Silence, not stale ring content.  The lead words
			 * below still advance: the device timeline must stay
			 * fed while the host stream recovers.  Before the
			 * seed anchors the counter the lead words are not
			 * placed against the device timeline, so ring data
			 * waits for the anchor. */
			memset(dst, 0, len);
		}
		if (frames[i]) {
			u32 base = s->out_counter;

			if (chip->ucx) {
				for (f = 0; f < frames[i]; f++)
					dst[f * fwords] = cpu_to_le32(
						(base + f) %
						ffuc_iso_wrap(chip));
			} else {
				dst[0] = cpu_to_le32(base);
			}
			s->out_counter += frames[i];
			/* The lag watermarks describe the anchored counter
			 * only: priming packets before the seed carry lead
			 * words that are not placed against the device
			 * timeline. */
			if (s->out_counter_valid) {
				unsigned int wrap = ffuc_iso_wrap(chip);
				int lag = (int)((chip->in_end + wrap -
						 s->out_counter) % wrap);

				if (lag < chip->out_lag_min)
					chip->out_lag_min = lag;
				if (lag > chip->out_lag_max)
					chip->out_lag_max = lag;
				if (i == 0) {
					u64 x = chip->out_pkts;

					chip->lag_n++;
					chip->lag_sx += x;
					chip->lag_sy += lag;
					chip->lag_sxx += x * x;
					chip->lag_sxy += x * lag;
					chip->lag_syy += (s64)lag * lag;
				}
			}
		}
		off += len;
	}
	urb->transfer_buffer_length = off;
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	/* Report the XRUN to ALSA so the application recovers through the
	 * normal prepare/start path.  The engine keeps running and feeds the
	 * device silence; only the host PCM stream stops.  Called outside the
	 * stream lock: snd_pcm_stop_xrun takes it itself. */
	if (stop)
		snd_pcm_stop_xrun(subs);
	if (elapsed)
		ffuc_period_elapsed(chip, SNDRV_PCM_STREAM_PLAYBACK);
}

/*
 * ISO URB continuity.  start_frame and number_of_packets are in the
 * units the HCD reports; the watchdog only compares successive URBs, so
 * any consistent unit cancels.  The first log line of a run records both
 * values so the unit can be confirmed from the bench.
 */
static bool ffuc_iso_contiguous(struct ffuc_ep *ep, struct urb *urb)
{
	int expected = ep->next_start;

	{
		static unsigned int start_logged;
		if (start_logged < 12) {
			start_logged++;
			dev_dbg(&ep->chip->dev->dev,
				"iso %s ep%d: start_frame %u packets %u delta %d\n",
				ep->dir ? "cap" : "play", ep->index,
				urb->start_frame, urb->number_of_packets,
				ep->last_start < 0 ? -1 :
				(int)(urb->start_frame - ep->last_start));
			ep->last_start = urb->start_frame;
		}
	}
	ep->next_start = (urb->start_frame + urb->number_of_packets) &
			 FFUC_ISO_FRAME_MASK;
	return expected < 0 ||
	       ((urb->start_frame - expected) & FFUC_ISO_FRAME_MASK) == 0;
}

static void ffuc_stop_running_streams(struct ffuc *chip)
{
	int dir;

	for (dir = 0; dir < 2; dir++) {
		struct snd_pcm_substream *subs = chip->streams[dir].substream;

		if (subs)
			snd_pcm_stop_xrun(subs);
	}
}

static void ffuc_track_playback_urb(struct ffuc *chip, struct urb *urb)
{
	s64 expected, delta;
	ktime_t now = ktime_get();

	if (urb->status)
		atomic_inc(&chip->out_status_errors);
	if (urb->actual_length != urb->transfer_buffer_length)
		atomic_inc(&chip->out_shorts);
	atomic_inc(&chip->out_urbs);
	if (!chip->rate || !READ_ONCE(chip->engine_running))
		return;
	expected = (s64)chip->urb_frames * NSEC_PER_SEC / chip->rate;
	if (chip->last_out_complete) {
		delta = ktime_to_ns(ktime_sub(now, chip->last_out_complete));
		if (delta > expected * 3)
			atomic_inc(&chip->out_gaps);
	}
	chip->last_out_complete = now;
}

static void ffuc_track_capture_urb(struct ffuc *chip, struct urb *urb)
{
	s64 expected, delta;
	ktime_t now = ktime_get();

	if (urb->status)
		atomic_inc(&chip->in_status_errors);
	atomic_inc(&chip->in_urbs);
	if (!chip->rate || !READ_ONCE(chip->engine_running))
		return;
	expected = (s64)chip->urb_frames * NSEC_PER_SEC / chip->rate;
	if (chip->last_in_complete) {
		delta = ktime_to_ns(ktime_sub(now, chip->last_in_complete));
		if (delta > expected * 3)
			atomic_inc(&chip->in_gaps);
	}
	chip->last_in_complete = now;
}

static int ffuc_stats_show(struct seq_file *m, void *v)
{
	struct ffuc *chip = m->private;

	seq_printf(m, "out_urbs %d\n", atomic_read(&chip->out_urbs));
	seq_printf(m, "out_gaps %d\n", atomic_read(&chip->out_gaps));
	seq_printf(m, "out_shorts %d\n", atomic_read(&chip->out_shorts));
	seq_printf(m, "out_status_errors %d\n",
		   atomic_read(&chip->out_status_errors));
	seq_printf(m, "in_urbs %d\n", atomic_read(&chip->in_urbs));
	seq_printf(m, "in_gaps %d\n", atomic_read(&chip->in_gaps));
	seq_printf(m, "in_shorts %d\n", atomic_read(&chip->in_shorts));
	seq_printf(m, "in_status_errors %d\n",
		   atomic_read(&chip->in_status_errors));
	seq_printf(m, "iso_out capacity_err %d pkt_len_err %d pkt_status %d fb_priming %d fb_ahead %d fb_stale %d fb_only_urbs %d\n",
		   atomic_read(&chip->out_capacity_err),
		   atomic_read(&chip->out_pkt_len_err),
		   atomic_read(&chip->out_pkt_status),
		   atomic_read(&chip->out_fb_priming),
		   atomic_read(&chip->out_fb_ahead),
		   atomic_read(&chip->out_fb_stale),
		   atomic_read(&chip->fb_only_urbs));
	seq_printf(m, "iso_in status_err %d short %d misaligned %d frames_mismatch %d pos_gap %d\n",
		   atomic_read(&chip->in_status_err),
		   atomic_read(&chip->in_short),
		   atomic_read(&chip->in_misaligned),
		   atomic_read(&chip->in_frames_mismatch),
		   atomic_read(&chip->in_pos_gap));
	seq_printf(m, "in_overruns %d\nout_underruns %d\n",
		   atomic_read(&chip->in_overruns),
		   atomic_read(&chip->out_underruns));
	seq_printf(m, "in_bad_length %d\nout_bad_length %d\n",
		   atomic_read(&chip->in_bad_length),
		   atomic_read(&chip->out_bad_length));
	seq_printf(m, "hw_ptr_mismatch %d\ndelta_zero %d\n",
		   atomic_read(&chip->hw_ptr_mismatch),
		   atomic_read(&chip->delta_zero));
	seq_printf(m, "urb_errors %d\n", atomic_read(&chip->urb_errors));
	seq_printf(m, "recoveries %u\n", chip->recoveries);
	seq_printf(m, "events_dropped %d\n", atomic_read(&chip->events_dropped));
	seq_printf(m, "meter_urbs %d\n", atomic_read(&chip->meter_urbs));
	seq_printf(m, "meter_paused %u\nmeter_pairs %u\nmeter_realigns %u\n",
		   chip->meter_paused, chip->meter_pairs, chip->meter_realigns);
	seq_printf(m, "urb_frames %u\nnurbs %u\n", chip->urb_frames,
		   chip->nurbs);
	seq_printf(m, "ring_min_filled %u\nring_missed %d\nring_starved %d\nring_max_stall_ns %llu\n",
		   chip->ring_min_filled, atomic_read(&chip->ring_missed),
		   atomic_read(&chip->ring_starved),
		   (unsigned long long)chip->ring_max_stall_ns);
	seq_printf(m, "gate deferred %d overdue %d max_pending %u\n",
		   atomic_read(&chip->signal_deferred),
		   atomic_read(&chip->signal_overdue),
		   READ_ONCE(chip->max_pending));
	seq_printf(m, "out_lag_min %d\nout_lag_max %d\n",
		   chip->out_lag_min, chip->out_lag_max);
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(ffuc_stats);

static int ffuc_clock_show(struct seq_file *m, void *v)
{
	struct ffuc *chip = m->private;
	unsigned long flags;
	u64 anchor, ns, pos, win_ns, win_pos, frames, frac;
	u32 phase;
	unsigned int rate;
	bool valid;

	spin_lock_irqsave(&chip->fb_lock, flags);
	anchor = chip->ts_anchor_ns;
	ns = chip->ts_ns;
	pos = chip->ts_pos;
	phase = chip->ts_phase;
	frac = chip->ts_frac_q32;
	win_ns = chip->ts_win_ns;
	win_pos = chip->ts_win_pos;
	frames = chip->ts_frames;
	rate = chip->ts_rate_meas;
	valid = chip->ts_pos_valid;
	spin_unlock_irqrestore(&chip->fb_lock, flags);

	seq_printf(m, "ts_valid %d\n", valid);
	seq_printf(m, "ts_ns %llu\nts_pos %llu\nts_phase %u\nts_frac_q32 %llu\n",
		   (unsigned long long)ns, (unsigned long long)pos, phase,
		   (unsigned long long)chip->ts_frac_q32);
	seq_printf(m, "rate_meas %u\n", rate);
	seq_printf(m, "win_ns %llu\nwin_pos %llu\n",
		   (unsigned long long)win_ns, (unsigned long long)win_pos);
	seq_printf(m, "anchor_ns %llu\nts_frames %llu\n",
		   (unsigned long long)anchor, (unsigned long long)frames);
	seq_printf(m, "anchor_resync %d\n",
		   atomic_read(&chip->in_anchor_resync));
	seq_printf(m, "tps_q32 %llu\n", (unsigned long long)chip->ts_tps_q32);
	seq_printf(m, "in_total %llu in_end %u valid %d\n",
		   (unsigned long long)chip->in_total, chip->in_end,
		   chip->in_total_valid);
	seq_printf(m, "seed span %lld pos_q32 %llu in %llu out %llu in_end %u counter %u\n",
		   (long long)chip->ts_seed_span,
		   (unsigned long long)chip->ts_seed_pos_q32,
		   (unsigned long long)chip->ts_seed_in,
		   (unsigned long long)chip->ts_seed_out,
		   chip->ts_seed_in_end, chip->ts_seed_counter);
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(ffuc_clock);

static int ffuc_ringtrace_show(struct seq_file *m, void *v)
{
	struct ffuc *chip = m->private;
	struct snd_pcm_substream *subs =
		READ_ONCE(chip->streams[SNDRV_PCM_STREAM_PLAYBACK].substream);
	struct ffuc_ring_rec *snap;
	unsigned long flags;
	u32 head, n, i;

	snap = kmalloc_array(FFUC_RING_TRACE, sizeof(*snap), GFP_KERNEL);
	if (!snap)
		return -ENOMEM;
	if (subs)
		snd_pcm_stream_lock_irqsave(subs, flags);
	head = chip->ring_head;
	n = min(head, (u32)FFUC_RING_TRACE);
	for (i = 0; i < n; i++)
		snap[i] = chip->ring_trace[(head - n + i) % FFUC_RING_TRACE];
	if (subs)
		snd_pcm_stream_unlock_irqrestore(subs, flags);

	seq_printf(m, "# %u completions total, last %u shown\n", head, n);
	seq_printf(m, "# min_filled %u missed %d starved %d max_stall_ns %llu\n",
		   chip->ring_min_filled, atomic_read(&chip->ring_missed),
		   atomic_read(&chip->ring_starved),
		   (unsigned long long)chip->ring_max_stall_ns);
	seq_puts(m, "# ktime_ns appl hw status_hw filled total flags\n");
	for (i = 0; i < n; i++) {
		const struct ffuc_ring_rec *r = &snap[i];

		seq_printf(m, "%llu %llu %llu %u %u %u %u\n",
			   (unsigned long long)r->ktime_ns,
			   (unsigned long long)r->appl,
			   (unsigned long long)r->hw, r->status_hw,
			   r->filled, r->total, r->flags);
	}
	kfree(snap);
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(ffuc_ringtrace);

static void ffuc_urb_complete(struct urb *urb)
{
	struct ffuc_urb *u = urb->context;
	struct ffuc_ep *ep = u->ep;
	struct ffuc *chip = ep->chip;
	int ret;

	if (urb->status != -ENOENT && urb->status != -ECONNRESET &&
	    urb->status != -ESHUTDOWN && urb->status != -ENODEV) {
		if (ep->dir == SNDRV_PCM_STREAM_PLAYBACK)
			ffuc_track_playback_urb(chip, urb);
		else
			ffuc_track_capture_urb(chip, urb);
	}

	if (u->frames && ep->dir == SNDRV_PCM_STREAM_PLAYBACK)
		atomic_sub(u->frames, &chip->streams[
			SNDRV_PCM_STREAM_PLAYBACK].inflight);

	switch (urb->status) {
	case 0:
		atomic_set(&chip->urb_errors, 0);
		break;
	case -ENOENT:
	case -ECONNRESET:
	case -ESHUTDOWN:
	case -ENODEV:
		return;
	case -EXDEV:
		if (chip->iso)
			break;
		fallthrough;
	default:
		dev_dbg_ratelimited(&chip->dev->dev, "%s URB status %d\n",
				    ep->dir ? "IN" : "OUT", urb->status);
		if (atomic_inc_return(&chip->urb_errors) > FFUC_MAX_URB_ERRORS) {
			dev_err(&chip->dev->dev, "too many URB errors, stopping\n");
			goto broken;
		}
		if (!chip->iso) {
			if (ep->dir == SNDRV_PCM_STREAM_PLAYBACK)
				memset(urb->transfer_buffer, 0,
				       urb->transfer_buffer_length);
			goto resubmit;
		}
	}

	if (!READ_ONCE(chip->engine_running))
		return;

	if (ep->dir == SNDRV_PCM_STREAM_CAPTURE) {
		if (chip->iso)
			ffuc_iso_capture_urb(ep, urb);
		else
			ffuc_capture_urb(ep, urb);
	} else {
		if (chip->iso)
			ffuc_iso_playback_fill(ep, urb);
		else
			ffuc_playback_urb(ep, urb);
	}

resubmit:
	if (!READ_ONCE(chip->engine_running))
		return;
	ret = usb_submit_urb(urb, GFP_ATOMIC);
	if (ret == 0) {
		if (!ep->dir) {
			struct ffuc_urb *uu = urb->context;

			if (uu && uu->frames)
				atomic_add(uu->frames,
					   &chip->streams[SNDRV_PCM_STREAM_PLAYBACK].inflight);
		}
		if (!READ_ONCE(chip->iso_frames_ok) ||
		    ffuc_iso_contiguous(ep, urb))
			return;
		dev_dbg_ratelimited(&chip->dev->dev, "%s URB queue ran dry\n",
				    ep->dir ? "IN" : "OUT");
		goto broken;
	}
	if (ret == -EPERM || ret == -ENOENT || ret == -ENODEV ||
	    ret == -ESHUTDOWN || !READ_ONCE(chip->engine_running))
		return;
	dev_err_ratelimited(&chip->dev->dev, "URB resubmit failed: %d\n", ret);
broken:
	if (!READ_ONCE(chip->engine_broken)) {
		WRITE_ONCE(chip->engine_broken, true);
		queue_work(system_unbound_wq, &chip->recover_work);
	}
}

static void ffuc_status_complete(struct urb *urb);

static int ffuc_status_submit(struct ffuc *chip)
{
	int ret;

	if (!chip->status_intf || chip->status_active ||
	    READ_ONCE(chip->disconnected))
		return 0;
	if (chip->desc->status_bulk)
		usb_fill_bulk_urb(chip->status_urb, chip->dev,
				  usb_rcvbulkpipe(chip->dev,
						  chip->desc->status_ep),
				  chip->status_buf, FFUC_STATUS_LEN,
				  ffuc_status_complete, chip);
	else
		usb_fill_int_urb(chip->status_urb, chip->dev,
				 usb_rcvintpipe(chip->dev,
						chip->desc->status_ep),
				 chip->status_buf, FFUC_STATUS_LEN,
				 ffuc_status_complete, chip, 1);
	WRITE_ONCE(chip->status_armed_at,
		   jiffies + msecs_to_jiffies(FFUC_STATUS_GRACE_MS));
	ret = usb_submit_urb(chip->status_urb, GFP_ATOMIC);
	if (!ret)
		chip->status_active = true;
	return ret;
}

static void ffuc_status_complete(struct urb *urb)
{
	struct ffuc *chip = urb->context;
	bool latched;
	int ret;

	chip->status_active = false;

	switch (urb->status) {
	case 0:
		break;
	case -ENOENT:
	case -ECONNRESET:
	case -ESHUTDOWN:
	case -ENODEV:
		return;
	default:
		goto resubmit;
	}

	if (urb->actual_length >= FFUC_STATUS_LEN) {
		memcpy(chip->last_status, chip->status_buf, FFUC_STATUS_LEN);
		ffuc_event_put(chip, FFUC_EV_STATUS, chip->status_buf,
			       min_t(unsigned int, urb->actual_length,
				     FFUC_EVENT_MAX), 0);
	}

	latched = (chip->desc->quirks & FFUC_QUIRK_STATUS_LATCH) &&
		  chip->last_status[3] & FFUC_STATUS_OUT_LATCHED;
	if (latched != READ_ONCE(chip->out_latched)) {
		WRITE_ONCE(chip->out_latched, latched);
		ffuc_event_put(chip, FFUC_EV_LATCH, NULL, 0,
			       latched ? 1 : 0);
	}

	if (latched &&
	    time_after(jiffies, READ_ONCE(chip->status_armed_at)) &&
	    READ_ONCE(chip->engine_running) &&
	    READ_ONCE(chip->streams[SNDRV_PCM_STREAM_PLAYBACK].running) &&
	    !READ_ONCE(chip->engine_broken)) {
		WRITE_ONCE(chip->engine_broken, true);
		queue_work(system_unbound_wq, &chip->recover_work);
		return;
	}

resubmit:
	/* The UC keeps the ticker open all the time (front-panel events);
	 * the UCX IF5 endpoint only reports stream-period ticks. */
	if (chip->ucx && !READ_ONCE(chip->engine_running))
		return;
	ret = ffuc_status_submit(chip);
	if (ret < 0 && ret != -EPERM)
		dev_dbg_ratelimited(&chip->dev->dev,
				    "status URB resubmit failed: %d\n", ret);
}

static void ffuc_kill_urbs(struct ffuc *chip, bool kill_status)
{
	int dir, e, i;

	for (dir = 0; dir < 2; dir++)
		for (e = 0; e < FFUC_MAX_EPS; e++)
			for (i = 0; i < chip->nurbs; i++)
				usb_kill_urb(chip->eps[dir][e].urbs[i].urb);
	if (kill_status) {
		usb_kill_urb(chip->status_urb);
		chip->status_active = false;
	}
}

static void ffuc_engine_stop(struct ffuc *chip)
{
	int i, j;

	if (!chip->engine_running)
		return;
	/* A restarted engine has a new frame counter; the ISO continuity
	 * watchdog must not compare across the restart. */
	for (i = 0; i < 2; i++)
		for (j = 0; j < FFUC_MAX_EPS; j++)
			chip->eps[i][j].next_start = -1;
	for (i = 0; i < 2; i++)
		for (j = 0; j < FFUC_MAX_EPS; j++)
			chip->eps[i][j].last_start = -1;
	dev_dbg(&chip->dev->dev,
		"engine stop: rate=%u session=%d\n",
		chip->rate, chip->session_reopen);
	/* Windows sends the session-close word while packets are still
	 * flowing (A2 t=13.03, stream dies at 13.3), and does not send a
	 * separate mute-all.  The UC close word is 0x13/0x000f; a later
	 * 0x13/0xffff would mute outputs the session-open 0x14/0x000f never
	 * unmutes. */
	if (!READ_ONCE(chip->disconnected)) {
		chip->desc->ops->stream_stop(chip, ffuc_mode_channels(chip,
								     chip->rate));
		if (chip->model != FFUC_MODEL_UC)
			chip->desc->ops->hw_mute(chip, true);
	}
	WRITE_ONCE(chip->engine_running, false);
	ffuc_kill_urbs(chip, chip->ucx || chip->iso);
	/* The kill is synchronous: every in-flight frame has been
	 * subtracted by its completion.  Rebase the counter for the next
	 * engine run. */
	atomic_set(&chip->streams[SNDRV_PCM_STREAM_PLAYBACK].inflight, 0);
	atomic_set(&chip->streams[SNDRV_PCM_STREAM_CAPTURE].inflight, 0);
	chip->engine_stopped_at = jiffies ?: 1;
	dev_dbg(&chip->dev->dev, "engine stopped\n");
}

static void ffuc_int_setup_urbs(struct ffuc *chip, unsigned int bytes)
{
	const struct ffuc_mode *m = chip->mode;
	unsigned int in_bytes = bytes;
	int dir, e, i;

	/* The Windows IF5 receive path always uses full 1024-frame URBs. */
	if (chip->ucx && !chip->iso)
		in_bytes = chip->base_urb_frames * ffuc_max_words(chip) *
			   FFUC_SAMPLE_BYTES;

	for (dir = 0; dir < 2; dir++) {
		for (e = 0; e < m->neps; e++) {
			struct ffuc_ep *ep = &chip->eps[dir][e];
			unsigned int pipe;
			unsigned int len;

			ep->buf_size = chip->urb_bytes_max;
			if (dir == SNDRV_PCM_STREAM_PLAYBACK)
				pipe = usb_sndintpipe(chip->dev, 1 + 2 * e);
			else
				pipe = usb_rcvintpipe(chip->dev, 2 + 2 * e);
			len = dir == SNDRV_PCM_STREAM_PLAYBACK ? bytes :
							       in_bytes;

			for (i = 0; i < chip->nurbs; i++) {
				struct ffuc_urb *u = &ep->urbs[i];

				usb_fill_int_urb(u->urb, chip->dev, pipe, u->buf,
						 len, ffuc_urb_complete, u, 1);
				u->urb->transfer_dma = u->dma;
				u->urb->transfer_flags |= URB_NO_TRANSFER_DMA_MAP;
				if (dir == SNDRV_PCM_STREAM_PLAYBACK) {
					memset(u->buf, 0, len);
					/* An interrupt OUT URB always carries
					 * urb_frames frames; the in-flight
					 * accounting uses that constant. */
					u->frames = chip->urb_frames;
				}
			}
		}
	}
}

static void ffuc_iso_setup_urbs(struct ffuc *chip)
{
	const struct ffuc_mode *m = chip->mode;
	int dir, i, p;

	chip->in_pkts = chip->out_pkts = 0;
	chip->streams[SNDRV_PCM_STREAM_PLAYBACK].out_counter_valid = false;
	chip->iso_frames_ok = false;
	chip->in_end = 0;
	chip->in_start_resync = false;
	chip->fb_acc = 0;
	chip->out_prime_acc = 0;
	chip->fb_delay = (chip->nurbs + 1) * chip->iso_packets;
	chip->out_lag_min = 0x7fffffff;
	chip->out_lag_max = -0x7fffffff;
	/* Device timestamp: the stream-side state restarts with the device
	 * stream; the start-frame anchor re-anchors too, because the frame
	 * index restarts at the first submitted URB. */
	chip->in_total_valid = false;
	chip->in_total = 0;
	chip->ts_pos_valid = false;
	chip->ts_pos = 0;
	chip->ts_ns = 0;
	chip->ts_phase = 0;
	chip->ts_rate_meas = 0;
	chip->ts_win_ns = 0;
	chip->ts_win_pos = 0;
	chip->ts_frames = 0;
	chip->ts_anchor_valid = false;
	chip->ts_seed_span = 0;
	chip->ts_seed_pos_q32 = 0;
	chip->ts_seed_in = 0;
	chip->ts_seed_out = 0;
	chip->ts_seed_in_end = 0;
	chip->ts_seed_counter = 0;
	chip->ts_frac_q32 = 0;
	chip->ts_tps_q32 = div64_u64(60000000ull << 32, chip->rate);
	/* Per-run diagnostics: a stop line describes this stream only. */
	memset(chip->iso_frames_hist, 0, sizeof(chip->iso_frames_hist));
	ffuc_cadence_reset(&chip->cad);
	atomic_set(&chip->in_hdr_only, 0);
	atomic_set(&chip->in_short, 0);
	atomic_set(&chip->in_misaligned, 0);
	atomic_set(&chip->in_status_err, 0);
	atomic_set(&chip->in_frames_mismatch, 0);
	atomic_set(&chip->in_pos_gap, 0);
	atomic_set(&chip->out_fb_priming, 0);
	atomic_set(&chip->out_fb_ahead, 0);
	atomic_set(&chip->out_fb_stale, 0);
	atomic_set(&chip->out_capacity_err, 0);
	atomic_set(&chip->out_pkt_len_err, 0);
	atomic_set(&chip->out_pkt_status, 0);
	atomic_set(&chip->fb_only_urbs, 0);
	chip->rate_mismatch_windows = 0;
	chip->lag_n = chip->lag_sx = chip->lag_sy = 0;
	chip->lag_sxx = chip->lag_sxy = chip->lag_syy = 0;
	WRITE_ONCE(chip->max_pending, 0);
	atomic_set(&chip->in_frames_overflow, 0);
	atomic_set(&chip->pcm_prepare_err, 0);
	atomic_set(&chip->pcm_trigger_err, 0);
	atomic_set(&chip->signal_deferred, 0);
	atomic_set(&chip->signal_overdue, 0);
	/* OUT data is placed this many frames after the IN position it
	 * mirrors.  The UCX needs OUT_LEAD (60) frames of margin at the
	 * unit; the UC path used the vendor's 2 * max_frames - 2 margin.
	 * Reduce modulo the wrap (8192 at 1x). */
	chip->out_lead = (DIV_ROUND_CLOSEST(chip->fb_delay * chip->rate,
					    8000) +
			  (chip->ucx ? 60 : 2 * m->max_frames - 2)) %
			 ffuc_iso_wrap(chip);

	for (dir = 0; dir < 2; dir++) {
		struct ffuc_ep *ep = &chip->eps[dir][0];

		ep->next_start = -1;
		ep->buf_size = chip->urb_bytes_max;
		for (i = 0; i < chip->nurbs; i++) {
			struct urb *urb = ep->urbs[i].urb;

			urb->dev = chip->dev;
			urb->pipe = dir == SNDRV_PCM_STREAM_PLAYBACK ?
				    usb_sndisocpipe(chip->dev, chip->out_ep) :
				    usb_rcvisocpipe(chip->dev, chip->in_ep);
			urb->interval = 1;
			urb->transfer_flags = URB_ISO_ASAP |
					      URB_NO_TRANSFER_DMA_MAP;
			urb->transfer_buffer = ep->urbs[i].buf;
			urb->transfer_dma = ep->urbs[i].dma;
			urb->number_of_packets = chip->iso_packets;
			urb->complete = ffuc_urb_complete;
			urb->context = &ep->urbs[i];
			if (dir == SNDRV_PCM_STREAM_PLAYBACK) {
				/* The fill's delivery check judges this URB's
				 * previous submission.  The URBs are reused across
				 * engine runs: a URB killed at the previous stop
				 * still carries that run's descriptors, and a
				 * cancelled delivery is not a transport error.
				 * Clear them so the first fill of this run counts
				 * only this run's deliveries. */
				for (p = 0; p < chip->iso_packets; p++) {
					urb->iso_frame_desc[p].offset = 0;
					urb->iso_frame_desc[p].length = 0;
					urb->iso_frame_desc[p].actual_length = 0;
					urb->iso_frame_desc[p].status = 0;
				}
				ffuc_iso_playback_fill(ep, urb);
				continue;
			}
			urb->transfer_buffer_length = chip->iso_packets *
						      m->in_mps;
			for (p = 0; p < chip->iso_packets; p++) {
				urb->iso_frame_desc[p].offset = p * m->in_mps;
				urb->iso_frame_desc[p].length = m->in_mps;
			}
		}
	}
}

static void ffuc_iso_align(struct ffuc *chip)
{
	struct ffuc_ep *ein = &chip->eps[SNDRV_PCM_STREAM_CAPTURE][0];
	struct ffuc_ep *eout = &chip->eps[SNDRV_PCM_STREAM_PLAYBACK][0];
	int in = ein->urbs[0].urb->start_frame;
	int out = eout->urbs[0].urb->start_frame;
	int diff = out - in;
	bool ok = true;
	unsigned long flags;
	int i;

	for (i = 0; i < chip->nurbs; i++) {
		ok &= ffuc_iso_contiguous(ein, ein->urbs[i].urb);
		ok &= ffuc_iso_contiguous(eout, eout->urbs[i].urb);
	}
	if (!ok || abs(diff) > 64)
		diff = 0;
	spin_lock_irqsave(&chip->fb_lock, flags);
	chip->out_lead = (chip->out_lead + ffuc_iso_wrap(chip) +
			  diff * (int)chip->rate / 8000) %
			 ffuc_iso_wrap(chip);
	spin_unlock_irqrestore(&chip->fb_lock, flags);
	WRITE_ONCE(chip->iso_frames_ok, ok);
	dev_dbg(&chip->dev->dev, "start frames IN %d OUT %d%s, OUT lead %u\n",
		in, out, ok ? "" : " (not contiguous, unchecked)", chip->out_lead);
}

static int ffuc_engine_start(struct ffuc *chip)
{
	const struct ffuc_mode *m = chip->mode;
	unsigned int bytes;
	int e, i, ret;

	if (chip->engine_running)
		return 0;
	/* Fresh continuity expectation for a fresh engine. */
	for (e = 0; e < 2; e++)
		for (i = 0; i < FFUC_MAX_EPS; i++) {
			chip->eps[e][i].next_start = -1;
			chip->eps[e][i].last_start = -1;
		}
	if (READ_ONCE(chip->disconnected))
		return -ENODEV;

	/* Fresh ring-fill watermarks for a fresh engine; the trace ring
	 * keeps its history across runs. */
	chip->ring_min_filled = UINT_MAX;
	chip->ring_last_appl = 0;
	chip->ring_last_appl_ts = 0;
	chip->ring_prev_ts = 0;
	chip->ring_max_stall_ns = 0;
	atomic_set(&chip->ring_missed, 0);
	atomic_set(&chip->ring_starved, 0);

	chip->urb_frames = ffuc_urb_frames(chip, chip->rate);
	if (chip->iso)
		bytes = chip->iso_packets * max(m->in_mps, m->out_mps);
	else
		bytes = chip->urb_frames *
			max(m->words, ffuc_mode_out_words(m)) *
			FFUC_SAMPLE_BYTES;
	if (WARN_ON(bytes > chip->urb_bytes_max))
		return -EINVAL;
	/* PipeWire reconfigures both nodes for a new quantum by closing and
	 * reopening them within milliseconds.  Windows leaves seconds
	 * between a session close and the next open; a start that follows
	 * the close word at once (interface reselect, session words) leaves
	 * a UC_WIN_RME unit dead in both directions until the module is
	 * reloaded.  100 ms was enough on the bench; keep a margin.  The
	 * settle is a Windows-personality property (FFUC_QUIRK_WIN_SESSION);
	 * the AP personality restarts without it. */
	if ((chip->desc->quirks & FFUC_QUIRK_WIN_SESSION) &&
	    chip->engine_stopped_at) {
		unsigned long ready = chip->engine_stopped_at +
				      msecs_to_jiffies(FFUC_RESTART_SETTLE_MS);

		if (time_before(jiffies, ready))
			msleep(jiffies_to_msecs(ready - jiffies) + 1);
	}

	/* alt 0 -> alt N reselect clears the UCX IF5 output latch. */
	if (chip->desc->quirks & FFUC_QUIRK_ALT_RESELECT) {
		ret = usb_set_interface(chip->dev, chip->audio_iface, 0);
		if (ret < 0)
			return ret;
		chip->cur_alt = 0;
	}
	/* Windows never issues SET_INTERFACE for the UC, including across a
	 * 48 <-> 44.1 live rate change (both use alt 1).  Re-selecting the
	 * same alt on the rate-change restart resets the streaming endpoints
	 * after the session words and leaves both directions silent.  Only
	 * program a genuinely different alt (2x/4x, or first start). */
	{
		bool skip_alt = chip->model == FFUC_MODEL_UC &&
				chip->uc_session &&
				chip->cur_alt == m->alt;

		dev_dbg(&chip->dev->dev,
			"engine start: rate=%u alt=%u cur_alt=%u session=%d skip_alt=%d\n",
			chip->rate, m->alt, chip->cur_alt,
			chip->session_reopen, skip_alt);
		if (!skip_alt) {
			ret = usb_set_interface(chip->dev, chip->audio_iface,
						m->alt);
			if (ret < 0)
				return ret;
			chip->cur_alt = m->alt;
		}
	}
	/* Cold start: commit the rate on every start.  The device keeps its
	 * own rate across personality switches and prior sessions, and the
	 * init sequence does not set it, so a cached value cannot be
	 * trusted.  This point is reached only with nothing running, so the
	 * commit cannot glitch a live stream.  On a Windows-session unit the
	 * close word needs the Windows gap before the rate write (A2:
	 * 220 ms); writing right after the close wedges the unit.  The
	 * settle above already separated the close from this start. */
	if ((chip->desc->quirks & FFUC_QUIRK_WIN_SESSION) && chip->uc_session)
		msleep(200);
	ret = chip->desc->ops->set_rate(chip, chip->rate);
	dev_dbg(&chip->dev->dev,
		"engine start: rate commit %u Hz alt %u cur_alt %u ret=%d\n",
		chip->rate, m->alt, chip->cur_alt, ret);
	if (ret < 0)
		return ret;

	/* Windows sends the session-open trio before the stream starts, not
	 * after the URBs are submitted.  On the UC the trio is what opens the
	 * device's audio path at all; sending it after the endpoints are
	 * already running leaves the stream alive with silence in both
	 * directions.  The later stream_start call is a no-op for the UC once
	 * session_reopen has been cleared. */
	if (chip->model == FFUC_MODEL_UC && chip->uc_session) {
		/* Windows leaves seconds between a stream close and the next
		 * open (A2b: 6.64 -> 10.87).  A back-to-back reopen after the
		 * 0x13 close leaves the UC silent even when the rate did not
		 * change.  Give the close time to take effect.  The gap is a
		 * Windows-personality property; the AP personality keeps the
		 * session words without it. */
		if (chip->desc->quirks & FFUC_QUIRK_WIN_SESSION)
			msleep(200);
		ret = chip->desc->ops->stream_start(
			chip, ffuc_mode_channels(chip, chip->rate));
		if (ret < 0)
			return ret;
	}

	if (chip->iso)
		ffuc_iso_setup_urbs(chip);
	else
		ffuc_int_setup_urbs(chip, bytes);

	/* The device resets its capture stream counter after the host stops
	 * polling.  Let the reset complete before the first URB so the run
	 * starts on a clean zero-based timeline, not the reset's position-0
	 * placeholders and error microframe (DRIVER-DESIGN §4.3, §7). */
	if (chip->iso)
		msleep(FFUC_ISO_START_MARGIN_MS);

	atomic_set(&chip->urb_errors, 0);
	chip->last_out_complete = 0;
	chip->last_in_complete = 0;
	WRITE_ONCE(chip->engine_broken, false);
	/* A new engine run: the per-run diagnostics restart and the host
	 * re-bases its counter view on this epoch. */
	chip->run_seq++;
	WRITE_ONCE(chip->engine_running, true);
	for (i = 0; i < chip->nurbs; i++) {
		for (e = 0; e < m->neps; e++) {
			ret = usb_submit_urb(
				chip->eps[SNDRV_PCM_STREAM_CAPTURE][e].urbs[i].urb,
				GFP_KERNEL);
			if (ret < 0)
				goto error;
			ret = usb_submit_urb(
				chip->eps[SNDRV_PCM_STREAM_PLAYBACK][e].urbs[i].urb,
				GFP_KERNEL);
			if (ret < 0)
				goto error;
			/* The initial submits join the in-flight accounting
			 * (R4): every submitted playback frame is added here
			 * or at the completion-path resubmit, and subtracted
			 * at completion.  Without this add the first URBs'
			 * completions subtract frames that were never
			 * counted, and the steady-state delay reads near
			 * zero. */
			{
				struct ffuc_urb *u =
					chip->eps[SNDRV_PCM_STREAM_PLAYBACK][e]
						.urbs[i].urb->context;

				if (u->frames)
					atomic_add(u->frames,
						   &chip->streams[SNDRV_PCM_STREAM_PLAYBACK].inflight);
			}
		}
	}

	/* The UC already sent its session-open trio before the URBs.  The
	 * UCX still needs its stream_start after the URBs are submitted. */
	if (chip->model != FFUC_MODEL_UC) {
		ret = chip->desc->ops->stream_start(
			chip, ffuc_mode_channels(chip, chip->rate));
		if (ret < 0)
			goto error;
	}

	if (chip->iso)
		ffuc_iso_align(chip);

	WRITE_ONCE(chip->status_armed_at,
		   jiffies + msecs_to_jiffies(FFUC_STATUS_GRACE_MS));
	ret = ffuc_status_submit(chip);
	if (ret < 0)
		dev_warn_ratelimited(&chip->dev->dev,
				     "cannot watch the status endpoint: %d\n", ret);

	ret = chip->desc->ops->hw_mute(chip, false);
	if (ret < 0)
		goto error;

	dev_dbg(&chip->dev->dev,
		"engine started: %u Hz, alt %u, %u frames x %u URBs\n",
		chip->rate, m->alt, chip->urb_frames, chip->nurbs);
	return 0;

error:
	dev_err(&chip->dev->dev, "failed to start streaming: %d\n", ret);
	WRITE_ONCE(chip->engine_running, false);
	ffuc_kill_urbs(chip, chip->ucx || chip->iso);
	return ret;
}

static void ffuc_recover_work(struct work_struct *work)
{
	struct ffuc *chip = container_of(work, struct ffuc, recover_work);
	int ret;

	mutex_lock(&chip->mutex);
	if (READ_ONCE(chip->disconnected) || !chip->engine_running ||
	    !READ_ONCE(chip->engine_broken))
		goto unlock;

	chip->recoveries++;
	dev_warn_ratelimited(&chip->dev->dev,
			     "stream interrupted (output underrun or USB error), restarting (%u)\n",
			     chip->recoveries);
	ffuc_stop_running_streams(chip);
	ffuc_engine_stop(chip);
	if (chip->prepared_mask) {
		ret = ffuc_engine_start(chip);
		if (ret < 0)
			dev_err(&chip->dev->dev, "restart failed: %d\n", ret);
	}
unlock:
	mutex_unlock(&chip->mutex);
}

/* -- PCM ------------------------------------------------------------- */

static const struct snd_pcm_hardware ffuc_pcm_hw = {
	.info = SNDRV_PCM_INFO_MMAP |
		SNDRV_PCM_INFO_MMAP_VALID |
		SNDRV_PCM_INFO_INTERLEAVED |
		SNDRV_PCM_INFO_BLOCK_TRANSFER |
		SNDRV_PCM_INFO_RESUME |
		/* BATCH: the pointer advances at the period boundary (one
		 * URB), which is what the device delivers.  JOINT_DUPLEX:
		 * both directions share the one device clock. */
		SNDRV_PCM_INFO_BATCH |
		SNDRV_PCM_INFO_JOINT_DUPLEX,
	.formats = SNDRV_PCM_FMTBIT_FLOAT_LE | SNDRV_PCM_FMTBIT_S32_LE,
	.rates = SNDRV_PCM_RATE_KNOT,
	.rate_min = 32000,
	.rate_max = FFUC_MAX_RATE,
	.channels_min = 1,
	.channels_max = FFUC_MAX_CHANNELS,
	.buffer_bytes_max = 4 * 1024 * 1024,
	.period_bytes_min = 32,
	.period_bytes_max = 1024 * 1024,
	.periods_min = 2,
	.periods_max = 1024,
};

static int ffuc_rule_channels(struct snd_pcm_hw_params *params,
			      struct snd_pcm_hw_rule *rule)
{
	struct snd_pcm_substream *subs = rule->private;
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	struct snd_interval *r = hw_param_interval(params, SNDRV_PCM_HW_PARAM_RATE);
	struct snd_interval t = {
		.min = 1,
		.max = ffuc_max_channels(chip, subs->stream, r->min),
		.integer = 1,
	};

	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_CHANNELS), &t);
}

static int ffuc_rule_rate(struct snd_pcm_hw_params *params,
			  struct snd_pcm_hw_rule *rule)
{
	struct snd_pcm_substream *subs = rule->private;
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	unsigned int other = 1 << !subs->stream;
	struct snd_interval t = {
		.min = 32000,
		.max = min(chip->desc->max_rate, (unsigned int)FFUC_MAX_RATE),
		.integer = 1,
	};
	unsigned int locked = 0;

	if (READ_ONCE(chip->hw_params_mask) & other)
		locked = READ_ONCE(chip->rate);
	else if (ffuc_rate_supported(chip, READ_ONCE(fixed_rate)))
		locked = READ_ONCE(fixed_rate);
	if (locked) {
		t.min = max(t.min, locked);
		t.max = min(t.max, locked);
	}
	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_RATE), &t);
}

static int ffuc_rule_period(struct snd_pcm_hw_params *params,
			    struct snd_pcm_hw_rule *rule)
{
	struct ffuc *chip = rule->private;
	struct snd_interval *r = hw_param_interval(params, SNDRV_PCM_HW_PARAM_RATE);
	/*
	 * The ALSA period and the USB URB are separate buffers.  The period
	 * floor is one URB (48 frames at 1x, scaled at 2x/4x); the graph
	 * quantum follows at 48 frames and above.
	 */
	struct snd_interval t = {
		.min = ffuc_urb_frames(chip, r->min),
		.max = UINT_MAX,
		.integer = 1,
	};

	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_PERIOD_SIZE), &t);
}

static unsigned int ffuc_buffer_min(struct ffuc *chip, unsigned int rate)
{
	/*
	 * The ALSA ring must cover the USB URBs already in flight plus two
	 * more URBs of margin.  It must also cover the UCX output lead: at
	 * 96 frames the old 192-frame ring was smaller than the 204-frame
	 * lead, which is the 96-and-below crackle threshold.  Add two URBs
	 * of margin beyond the lead and keep the result URB-aligned.
	 */
	unsigned int urb = ffuc_urb_frames(chip, rate);
	unsigned int min = (chip->nurbs + 2) * urb;

	if (chip->ucx) {
		unsigned int fb_delay = (chip->nurbs + 1) * chip->iso_packets;
		unsigned int lead = DIV_ROUND_CLOSEST(fb_delay * rate, 8000) + 60;

		min = max(min, lead + 2 * urb);
	}

	/* roundup() rounds to any multiple; round_up() is a power-of-two
	 * bitmask and would mis-align a non-power-of-two URB. */
	return roundup(min, urb);
}

static int ffuc_rule_buffer(struct snd_pcm_hw_params *params,
			    struct snd_pcm_hw_rule *rule)
{
	struct ffuc *chip = rule->private;
	struct snd_interval *r = hw_param_interval(params, SNDRV_PCM_HW_PARAM_RATE);
	struct snd_interval t = {
		.min = ffuc_buffer_min(chip, r->min),
		.max = UINT_MAX,
		.integer = 1,
	};

	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_BUFFER_SIZE), &t);
}

/* The achievable rate/quantum envelope, reported through hwdep so userspace
 * presents exactly what the driver accepts.  Uses the same geometry functions
 * as the hw_params rules: one implementation, one owner. */
void ffuc_query_envelope(struct ffuc *chip, struct ffuc_hwdep_envelope *env)
{
	unsigned int i, n = 0;

	memset(env, 0, sizeof(*env));
	env->urb_frames = chip->base_urb_frames;
	env->nurbs = chip->nurbs;
	env->iso = chip->iso ? 1 : 0;
	for (i = 0; i < ARRAY_SIZE(ffuc_rates) && n < FFUC_HWDEP_MAX_RATES; i++) {
		unsigned int rate = ffuc_rates[i];

		if (!ffuc_rate_supported(chip, rate))
			continue;
		env->rates[n].rate = rate;
		env->rates[n].period_floor = ffuc_urb_frames(chip, rate);
		env->rates[n].buffer_min = ffuc_buffer_min(chip, rate);
		n++;
	}
	env->count = n;
}

static int ffuc_pcm_open(struct snd_pcm_substream *subs)
{
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	struct snd_pcm_runtime *rt = subs->runtime;
	int ret;

	if (READ_ONCE(chip->disconnected))
		return -ENODEV;

	rt->hw = ffuc_pcm_hw;
	ret = snd_pcm_hw_constraint_list(rt, 0, SNDRV_PCM_HW_PARAM_RATE,
					 &ffuc_rate_list);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_CHANNELS,
				  ffuc_rule_channels, subs,
				  SNDRV_PCM_HW_PARAM_RATE, -1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_RATE,
				  ffuc_rule_rate, subs,
				  SNDRV_PCM_HW_PARAM_CHANNELS, -1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_PERIOD_SIZE,
				  ffuc_rule_period, chip,
				  SNDRV_PCM_HW_PARAM_RATE, -1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_BUFFER_SIZE,
				  ffuc_rule_buffer, chip,
				  SNDRV_PCM_HW_PARAM_RATE, -1);
	if (ret < 0)
		return ret;
	return snd_pcm_hw_constraint_msbits(rt, 0, 32, 24);
}

static int ffuc_pcm_close(struct snd_pcm_substream *subs)
{
	return 0;
}

static int ffuc_pcm_hw_params(struct snd_pcm_substream *subs,
			      struct snd_pcm_hw_params *params)
{
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	unsigned int rate = params_rate(params);
	unsigned int self = 1 << subs->stream;
	int ret = 0;

	if (params_channels(params) > ffuc_max_channels(chip, subs->stream, rate))
		return -EINVAL;

	mutex_lock(&chip->mutex);
	if (rate != chip->rate || !chip->mode) {
		dev_dbg(&chip->dev->dev,
			"hw_params: stream=%s rate=%u old=%u engine=%d\n",
			subs->stream == SNDRV_PCM_STREAM_PLAYBACK ?
			"playback" : "capture",
			rate, chip->rate, chip->engine_running);
		if ((chip->hw_params_mask | chip->prepared_mask) & ~self) {
			ret = -EBUSY;
			goto unlock;
		}
		/* Stop before arming the session flags.  The first rate-change
		 * stop must not send the session-close word (A2 sends nothing
		 * between the last tone packet and the rate write); from then
		 * on every UC stop/open gets the session words. */
		ffuc_engine_stop(chip);
		if (chip->model == FFUC_MODEL_UC) {
			chip->session_reopen = true;
			chip->uc_session = true;
		}
		chip->prepared_mask &= ~self;
		chip->rate = rate;
		chip->mode = &chip->modes[ffuc_mode_index(rate)];
	}
	chip->hw_params_mask |= self;
unlock:
	mutex_unlock(&chip->mutex);
	return ret;
}

static int ffuc_pcm_hw_free(struct snd_pcm_substream *subs)
{
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	unsigned int self = 1 << subs->stream;

	mutex_lock(&chip->mutex);
	chip->hw_params_mask &= ~self;
	chip->prepared_mask &= ~self;
	if (!chip->prepared_mask)
		ffuc_engine_stop(chip);
	mutex_unlock(&chip->mutex);
	return 0;
}

static int ffuc_pcm_prepare(struct snd_pcm_substream *subs)
{
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	struct ffuc_stream *s = &chip->streams[subs->stream];
	int ret;

	mutex_lock(&chip->mutex);
	if (READ_ONCE(chip->engine_broken))
		ffuc_engine_stop(chip);
	ret = ffuc_engine_start(chip);
	if (ret == 0)
		chip->prepared_mask |= 1 << subs->stream;
	mutex_unlock(&chip->mutex);
	if (ret < 0) {
		atomic_inc(&chip->pcm_prepare_err);
		return ret;
	}

	snd_pcm_stream_lock_irq(subs);
	s->running = false;
	memset(s->pos, 0, sizeof(s->pos));
	s->hw_ptr = 0;
	s->period = 0;
	s->signalled = 0;
	s->deferrals = 0;
	s->out_starve_streak = 0;
	s->out_start_urbs = 0;
	if (subs->stream == SNDRV_PCM_STREAM_PLAYBACK)
		s->out_counter_valid = false;
	/* Preparation never zeroed the ring; inherit the last session's
	 * (or the allocator's) bytes.  Poison so that a recorded zero can
	 * never be confused with never-written memory.  Safe amplitude in
	 * both formats. */
	if (ffuc_poison && subs->runtime && subs->runtime->dma_area)
		memset32((u32 *)subs->runtime->dma_area, 0x00005A5AU,
			 subs->runtime->dma_bytes / 4);
	snd_pcm_stream_unlock_irq(subs);
	return 0;
}

static int ffuc_pcm_trigger(struct snd_pcm_substream *subs, int cmd)
{
	struct ffuc *chip = snd_pcm_substream_chip(subs);
	struct ffuc_stream *s = &chip->streams[subs->stream];
	int i;

	switch (cmd) {
	case SNDRV_PCM_TRIGGER_START:
		if (!(chip->prepared_mask & (1u << subs->stream))) {
			atomic_inc(&chip->pcm_trigger_err);
			return -EPIPE;
		}
		if (!READ_ONCE(chip->engine_running)) {
			atomic_inc(&chip->pcm_trigger_err);
			return -EIO;
		}
		memset(s->pos, 0, sizeof(s->pos));
		s->hw_ptr = 0;
		s->period = 0;
		s->signalled = 0;
		s->deferrals = 0;
		s->out_starve_streak = 0;
		s->out_start_urbs = FFUC_START_GRACE_URBS;
		s->out_counter_valid = false;
		s->running = true;
		/* q48 diagnostics: log the negotiated geometry once per
		 * start so a capture session can be matched to it. */
		dev_dbg(&chip->dev->dev,
			"%s start: rate %u, urbs %u x %u frames, buffer %lu, period %lu, stop_threshold %lu, start_threshold %lu, avail_min %lu, delay %lu\n",
			subs->stream ? "capture" : "playback", chip->rate,
			chip->nurbs, chip->urb_frames,
			subs->runtime->buffer_size, subs->runtime->period_size,
			subs->runtime->stop_threshold,
			subs->runtime->start_threshold,
			subs->runtime->control->avail_min,
			(unsigned long)subs->runtime->delay);
		return 0;
	case SNDRV_PCM_TRIGGER_RESUME:
		/*
		 * The stream was running when the system suspended and ALSA
		 * has already restored the hardware.  If the engine could not
		 * be restarted in ffuc_resume(), refuse the resume so ALSA
		 * leaves the stream suspended and userspace can re-prepare it
		 * instead of running a stream with no URBs.
		 */
		if (!(chip->prepared_mask & (1u << subs->stream)) ||
		    !READ_ONCE(chip->engine_running)) {
			atomic_inc(&chip->pcm_trigger_err);
			return -EIO;
		}
		memset(s->pos, 0, sizeof(s->pos));
		s->hw_ptr = 0;
		s->period = 0;
		s->signalled = 0;
		s->deferrals = 0;
		s->out_starve_streak = 0;
		s->out_start_urbs = FFUC_START_GRACE_URBS;
		s->out_counter_valid = false;
		s->running = true;
		return 0;
	case SNDRV_PCM_TRIGGER_STOP:
	case SNDRV_PCM_TRIGGER_SUSPEND:
		s->running = false;
		/* Gate proof, visible without root: the number of signals
		 * that would have announced zero completed periods, the
		 * published-pointer divergences, and the ring anomalies. */
		{
			/* The stop verdict judges the converged cadence mean
			 * (§4.3): a run stopped before the mean settled — the
			 * startup ramp, a short renegotiation fragment — has
			 * no cadence to judge, so no verdict is rendered.  The
			 * convergence is dynamic (successive windows agree),
			 * not a fixed sample count. */
			if (!atomic_read(&chip->in_frames_overflow) &&
			    chip->cad.stable_mean) {
				chip->iso_cadence = chip->cad.stable_mean;
				if (ffuc_cadence_stop_mismatch(&chip->cad,
							       chip->rate)) {
					atomic_inc(&chip->rate_mismatch);
					dev_warn_ratelimited(&chip->dev->dev,
							     "ISO cadence %u Hz disagrees with rate %u Hz\n",
							     chip->iso_cadence, chip->rate);
				}
			}
		}
		{
			u32 over = 0;

			for (i = 9; i < ARRAY_SIZE(chip->iso_frames_hist); i++)
				over += chip->iso_frames_hist[i];
			dev_dbg(&chip->dev->dev,
				"iso frames histogram [0..8,9+]: %u %u %u %u %u %u %u %u %u %u\n",
				chip->iso_frames_hist[0], chip->iso_frames_hist[1],
				chip->iso_frames_hist[2], chip->iso_frames_hist[3],
				chip->iso_frames_hist[4], chip->iso_frames_hist[5],
				chip->iso_frames_hist[6], chip->iso_frames_hist[7],
				chip->iso_frames_hist[8], over);
		}
		dev_dbg(&chip->dev->dev,
			"%s stop: delta_zero(cum) %d mismatch(cum) %d overruns %d/%d bad_length %d/%d fb_only %d fb %d/%d/%d cap %d pktlen %d/%d hdr0 %d lag %d..%d cadence %uHz err(cum) %d delay %u lead %u\n",
			subs->stream ? "capture" : "playback",
			atomic_read(&chip->delta_zero),
			atomic_read(&chip->hw_ptr_mismatch),
			atomic_read(&chip->in_overruns),
			atomic_read(&chip->out_underruns),
			atomic_read(&chip->in_bad_length),
			atomic_read(&chip->out_bad_length),
			atomic_read(&chip->fb_only_urbs),
			atomic_read(&chip->out_fb_priming),
			atomic_read(&chip->out_fb_ahead),
			atomic_read(&chip->out_fb_stale),
			atomic_read(&chip->out_capacity_err),
			atomic_read(&chip->out_pkt_len_err),
			atomic_read(&chip->out_pkt_status),
			atomic_read(&chip->in_hdr_only),
			chip->out_lag_min, chip->out_lag_max,
			chip->iso_cadence,
			atomic_read(&chip->rate_mismatch),
			chip->fb_delay, chip->out_lead);
		dev_dbg(&chip->dev->dev,
			"gate: deferred %d overdue %d max_pending %u overflow %d prep_err %d trig_err %d\n",
			atomic_read(&chip->signal_deferred),
			atomic_read(&chip->signal_overdue),
			READ_ONCE(chip->max_pending),
			atomic_read(&chip->in_frames_overflow),
			atomic_read(&chip->pcm_prepare_err),
			atomic_read(&chip->pcm_trigger_err));
		dev_dbg(&chip->dev->dev,
			"iso lag samples (one per OUT URB): n %llu sx %lld sy %lld sxx %lld sxy %lld syy %lld\n",
			(unsigned long long)chip->lag_n,
			(long long)chip->lag_sx, (long long)chip->lag_sy,
			(long long)chip->lag_sxx, (long long)chip->lag_sxy,
			(long long)chip->lag_syy);
		return 0;
	default:
		return -EINVAL;
	}
}

static snd_pcm_uframes_t ffuc_pcm_pointer(struct snd_pcm_substream *subs)
{
	struct ffuc *chip = snd_pcm_substream_chip(subs);

	/* The pointer is the driver's read/write position.  runtime->delay
	 * is the queue between it and the device: for playback, the frames
	 * handed to USB but not yet completed.  Capture in-flight frames are
	 * not accounted yet, so the capture delay is the ring fill.  ALSA
	 * adds hw_avail to it, so snd_pcm_delay()/htimestamp see the real
	 * queue (audit R4). */
	subs->runtime->delay = atomic_read(&chip->streams[subs->stream].inflight);

	return chip->streams[subs->stream].hw_ptr %
	       subs->runtime->buffer_size;
}

static int ffuc_pcm_sync_stop(struct snd_pcm_substream *subs)
{
	/* The stop is a flag change under the stream lock, and the URB
	 * kill is synchronous.  Taking the lock synchronises with a
	 * completion that is between its running check and its period
	 * signal, so nothing is pending when the core proceeds to
	 * hw_free/reconfigure (audit R6). */
	unsigned long flags;

	snd_pcm_stream_lock_irqsave(subs, flags);
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	return 0;
}

static const struct snd_pcm_ops ffuc_pcm_ops = {
	.open = ffuc_pcm_open,
	.close = ffuc_pcm_close,
	.hw_params = ffuc_pcm_hw_params,
	.hw_free = ffuc_pcm_hw_free,
	.prepare = ffuc_pcm_prepare,
	.trigger = ffuc_pcm_trigger,
	.pointer = ffuc_pcm_pointer,
	.sync_stop = ffuc_pcm_sync_stop,
};

static int ffuc_create_pcm(struct ffuc *chip)
{
	struct snd_pcm *pcm;
	int ret;

	ret = snd_pcm_new(chip->card, chip->desc->name, 0, 1, 1, &pcm);
	if (ret < 0)
		return ret;
	pcm->private_data = chip;
	strscpy(pcm->name, chip->desc->name, sizeof(pcm->name));
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &ffuc_pcm_ops);
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_CAPTURE, &ffuc_pcm_ops);
	/*
	 * ffuc_suspend() decides per direction whether a PCM needs to enter
	 * SNDRV_PCM_STATE_SUSPENDED.  Without this flag the ALSA core suspends
	 * every open PCM first, including prepared-but-stopped playback, which
	 * leaves some userspace graphs with a suspended sink after resume.
	 */
	pcm->no_device_suspend = true;
	snd_pcm_set_managed_buffer_all(pcm, SNDRV_DMA_TYPE_VMALLOC, NULL, 0, 0);

	chip->streams[SNDRV_PCM_STREAM_PLAYBACK].substream =
		pcm->streams[SNDRV_PCM_STREAM_PLAYBACK].substream;
	chip->streams[SNDRV_PCM_STREAM_CAPTURE].substream =
		pcm->streams[SNDRV_PCM_STREAM_CAPTURE].substream;
	chip->pcm = pcm;
	return 0;
}

/* -- USB driver ------------------------------------------------------ */

static int ffuc_alloc_urbs(struct ffuc *chip)
{
	int dir, e, i;

	chip->status_urb = usb_alloc_urb(0, GFP_KERNEL);
	chip->status_buf = kmalloc(FFUC_STATUS_LEN, GFP_KERNEL);
	if (!chip->status_urb || !chip->status_buf)
		return -ENOMEM;

	for (dir = 0; dir < 2; dir++) {
		for (e = 0; e < FFUC_MAX_EPS; e++) {
			struct ffuc_ep *ep = &chip->eps[dir][e];

			ep->chip = chip;
			ep->dir = dir;
			ep->index = e;
			for (i = 0; i < chip->nurbs; i++) {
				struct ffuc_urb *u = &ep->urbs[i];

				u->ep = ep;
				u->urb = usb_alloc_urb(chip->iso ?
						       FFUC_MAX_ISO_PACKETS : 0,
						       GFP_KERNEL);
				if (!u->urb)
					return -ENOMEM;
				u->buf = usb_alloc_coherent(chip->dev,
							    chip->urb_bytes_max,
							    GFP_KERNEL, &u->dma);
				if (!u->buf)
					return -ENOMEM;
			}
		}
	}
	return 0;
}

static void ffuc_free_urbs(struct ffuc *chip)
{
	int dir, e, i;

	if (chip->status_urb) {
		usb_kill_urb(chip->status_urb);
		usb_free_urb(chip->status_urb);
		chip->status_urb = NULL;
	}
	kfree(chip->status_buf);
	chip->status_buf = NULL;

	for (dir = 0; dir < 2; dir++) {
		for (e = 0; e < FFUC_MAX_EPS; e++) {
			for (i = 0; i < FFUC_MAX_URBS; i++) {
				struct ffuc_urb *u = &chip->eps[dir][e].urbs[i];

				if (u->urb) {
					usb_kill_urb(u->urb);
					usb_free_urb(u->urb);
					u->urb = NULL;
				}
				if (u->buf) {
					usb_free_coherent(chip->dev,
							  chip->urb_bytes_max,
							  u->buf, u->dma);
					u->buf = NULL;
				}
			}
		}
	}
}

static void ffuc_card_free(struct snd_card *card)
{
	struct ffuc *chip = card->private_data;

	cancel_work_sync(&chip->recover_work);
	ffuc_hwdep_cleanup(chip);
	ffuc_midi_cleanup(chip);
	if (chip->status_intf) {
		usb_driver_release_interface(&ffuc_driver, chip->status_intf);
		chip->status_intf = NULL;
	}
	ffuc_free_urbs(chip);
	usb_put_dev(chip->dev);
	mutex_lock(&devices_mutex);
	clear_bit(chip->dev_idx, &devices_used);
	mutex_unlock(&devices_mutex);
}

static int ffuc_hw_setup(struct ffuc *chip)
{
	int ret;

	ret = usb_set_interface(chip->dev, chip->audio_iface,
				chip->mode->alt);
	if (ret < 0)
		return ret;
	chip->cur_alt = chip->mode->alt;
	ret = chip->desc->ops->init(chip);
	if (ret < 0)
		return ret;
	ret = chip->desc->ops->set_rate(chip, chip->rate);
	if (ret < 0)
		return ret;
	return chip->desc->ops->hw_mute(chip, true);
}

static void ffuc_read_identity(struct ffuc *chip)
{
	u32 rev = 0;

	if (ffuc_vendor_recv(chip, FFUC_REQ_REVISION, 0, 0, &rev,
			     sizeof(rev)) == 0)
		chip->firmware = (le32_to_cpu(rev) >> 16) & 0xff;
	if (chip->dev->serial)
		strscpy(chip->serial, chip->dev->serial, sizeof(chip->serial));
}

static int ffuc_probe(struct usb_interface *intf,
		      const struct usb_device_id *usb_id)
{
	struct usb_device *dev = interface_to_usbdev(intf);
	const struct ffuc_device_desc *desc =
		(const struct ffuc_device_desc *)usb_id->driver_info;
	struct snd_card *card;
	struct ffuc *chip;
	unsigned int iface_no = intf->cur_altsetting->desc.bInterfaceNumber;
	unsigned int max_words;
	int dev_idx, ret;

	if (!desc)
		return -ENODEV;
	if (iface_no != desc->iface)
		return -ENODEV;
	/* Never bind a class-compliant (UAC2) interface.  In CC mode the
	 * device must be left to snd-usb-audio; this driver is only for the
	 * vendor-specific RME personality. */
	if (intf->cur_altsetting->desc.bInterfaceClass !=
	    USB_CLASS_VENDOR_SPEC)
		return -ENODEV;

	/* One card per physical UCX: bind either IF0 or IF5, never both. */
	if (desc->model == FFUC_MODEL_UCX &&
	    ((ucx_if5 && desc->transport != FFUC_XPORT_INTERRUPT) ||
	     (!ucx_if5 && desc->transport != FFUC_XPORT_ISO)))
		return -ENODEV;

	mutex_lock(&devices_mutex);
	for (dev_idx = 0; dev_idx < SNDRV_CARDS; dev_idx++)
		if (enable[dev_idx] && !test_bit(dev_idx, &devices_used))
			break;
	if (dev_idx >= SNDRV_CARDS) {
		mutex_unlock(&devices_mutex);
		return -ENODEV;
	}
	set_bit(dev_idx, &devices_used);
	mutex_unlock(&devices_mutex);

	ret = snd_card_new(&intf->dev, index[dev_idx], id[dev_idx], THIS_MODULE,
			   sizeof(*chip), &card);
	if (ret < 0)
		goto release_idx;

	chip = card->private_data;
	chip->card = card;
	chip->dev = usb_get_dev(dev);
	chip->intf = intf;
	chip->desc = desc;
	chip->model = desc->model;
	chip->iso = desc->transport == FFUC_XPORT_ISO;
	/* The Babyface-classic uses the UCX transport (ISO format, wrap,
	 * stream open/close words). */
	chip->ucx = desc->model == FFUC_MODEL_UCX ||
		    desc->model == FFUC_MODEL_BABYFACE_CLASSIC;
	chip->dev_idx = dev_idx;
	chip->audio_iface = desc->audio_iface;
	chip->out_ep = desc->out_ep;
	chip->in_ep = desc->in_ep;
	chip->modes = desc->modes;
	chip->debugfs = debugfs_create_dir(dev_name(&intf->dev), NULL);
	if (!IS_ERR_OR_NULL(chip->debugfs)) {
		debugfs_create_file("stats", 0444, chip->debugfs, chip,
				    &ffuc_stats_fops);
		debugfs_create_file("ringtrace", 0444, chip->debugfs, chip,
				    &ffuc_ringtrace_fops);
		debugfs_create_file("clock", 0444, chip->debugfs, chip,
				    &ffuc_clock_fops);
	}
	chip->ring_min_filled = UINT_MAX;
	mutex_init(&chip->mutex);
	mutex_init(&chip->ctl_lock);
	spin_lock_init(&chip->fb_lock);
	spin_lock_init(&chip->gate_lock);
	INIT_WORK(&chip->recover_work, ffuc_recover_work);
	init_waitqueue_head(&chip->event_wait);
	spin_lock_init(&chip->event_lock);
	atomic_set(&chip->event_seq, 0);
	atomic_set(&chip->events_dropped, 0);
	atomic_set(&chip->meter_urbs, 0);
	/* Meter phase is unknown until the device anchors it at power-up;
	 * the flag marks the first heartbeat of the session. */
	WRITE_ONCE(chip->meter_phase_lost, true);
	atomic_set(&chip->state_urbs, 0);
	atomic_set(&chip->hwdep_users, 0);
	if (kfifo_alloc(&chip->event_fifo, FFUC_EVENT_FIFO_SIZE, GFP_KERNEL)) {
		ret = -ENOMEM;
		goto free_card;
	}
	card->private_free = ffuc_card_free;

	chip->nurbs = clamp(nurbs, 2u, (unsigned int)FFUC_MAX_URBS);
	chip->base_urb_frames = clamp(round_up(
		(chip->ucx && !chip->iso) ? ucx_if5_frames : urb_frames, 8),
		8u, (chip->ucx && !chip->iso) ? 1024u : 256u);
	if (chip->nurbs * chip->base_urb_frames < FFUC_MIN_QUEUE_FRAMES) {
		chip->nurbs = DIV_ROUND_UP(FFUC_MIN_QUEUE_FRAMES,
					   chip->base_urb_frames);
		dev_dbg(&intf->dev, "using %u URBs to keep %u frames queued\n",
			chip->nurbs, FFUC_MIN_QUEUE_FRAMES);
	}
	chip->iso_packets = clamp(DIV_ROUND_UP(chip->base_urb_frames, 6), 1u,
				  (unsigned int)FFUC_MAX_ISO_PACKETS);
	max_words = ffuc_max_words(chip);
	if (chip->iso) {
		chip->urb_bytes_max = chip->iso_packets *
				      max(chip->modes[0].in_mps,
					  chip->modes[chip->desc->n_modes - 1].out_mps);
	} else {
		unsigned int i, span = 0;

		/* The largest interrupt URB of any rate band: at 1x the base
		 * frame count times 18 words, at 2x twice as many frames times
		 * 14 words, and so on. */
		for (i = 0; i < chip->desc->n_modes; i++) {
			unsigned int frames = (chip->ucx) ?
				chip->base_urb_frames :
				chip->base_urb_frames << i;

			span = max(span, frames *
				   max(chip->modes[i].words,
				       ffuc_mode_out_words(&chip->modes[i])));
		}
		chip->urb_bytes_max = span * FFUC_SAMPLE_BYTES;
		if (chip->iso) {
			const struct ffuc_mode *im;
			unsigned int k;

			for (k = 0; k < chip->desc->n_modes; k++) {
				im = &chip->modes[k];
				/* The feedback frames field is 5 bits and the
				 * packet arithmetic must stay inside it. */
				WARN_ON(im->max_frames > 31);
				WARN_ON(im->neps != 1);
				WARN_ON(im->max_frames * (im->words + 1) *
					FFUC_SAMPLE_BYTES > im->out_mps);
				WARN_ON(FFUC_ISO_IN_HEADER +
					im->max_frames * im->words *
					FFUC_SAMPLE_BYTES > im->in_mps);
			}
		}
	}
	/* The Windows IF5 driver reads 73,728-byte IN URBs. */
	if ((chip->desc->quirks & FFUC_QUIRK_IF5_PLAYBACK_MASK) &&
	    chip->urb_bytes_max < 73728)
		chip->urb_bytes_max = 73728;
	chip->rate = 48000;
	chip->mode = &chip->modes[0];
	dev_idx = -1;		/* released by ffuc_card_free() from now on */

	strscpy(card->driver, "FirefaceUSB", sizeof(card->driver));
	strscpy(card->shortname, desc->shortname, sizeof(card->shortname));
	strscpy(card->mixername, desc->shortname, sizeof(card->mixername));
	if (!id[chip->dev_idx])
		snd_card_set_id(card, desc->card_id);

	ffuc_midi_init(chip);
	ret = ffuc_alloc_urbs(chip);
	if (ret < 0)
		goto free_card;

	if (desc->status_iface != FFUC_IFACE_NONE) {
		chip->status_intf = usb_ifnum_to_if(dev, desc->status_iface);
		if (chip->status_intf &&
		    usb_driver_claim_interface(&ffuc_driver, chip->status_intf,
					       NULL) < 0) {
			dev_warn(&intf->dev,
				 "status interface busy, no underrun recovery\n");
			chip->status_intf = NULL;
		}
	}

	ffuc_read_identity(chip);
	chip->s32_samples = ffuc_wire_s32(chip);
	dev_info(&intf->dev, "firmware %u, %s samples on the wire\n",
		 chip->firmware, chip->s32_samples ? "S32" : "float");
	{
		u32 st = 0;

		if (ffuc_vendor_recv(chip, FFUC_REQ_STATUS, 0, 0, &st,
				     sizeof(st)) == 0)
			dev_info(&intf->dev,
				 "probe status 0x%08x rate index %u (device state before any write)\n",
				 le32_to_cpu(st), le32_to_cpu(st) >> 24);
	}
	snprintf(card->longname, sizeof(card->longname),
		 "RME %s (%s, firmware %u) at %s",
		 desc->name,
		 chip->iso ? "ISO transport" : "interrupt transport",
		 chip->firmware, dev_name(&dev->dev));

	ret = ffuc_hw_setup(chip);
	if (ret < 0) {
		dev_err(&intf->dev, "hardware init failed: %d\n", ret);
		goto free_card;
	}
	ret = ffuc_create_pcm(chip);
	if (ret < 0)
		goto free_card;
	ret = ffuc_hwdep_create(chip);
	if (ret < 0)
		goto free_card;
	ret = ffuc_midi_create(chip);
	if (ret < 0)
		dev_warn(&intf->dev, "no MIDI port: %d\n", ret);
	ret = ffuc_transport_setup(chip);
	if (ret < 0)
		dev_warn(&intf->dev, "raw control transports unavailable: %d\n",
			 ret);
	ret = snd_card_register(card);
	if (ret < 0)
		goto free_card;

	usb_set_intfdata(intf, chip);
	if (chip->status_intf && ffuc_status_submit(chip) < 0)
		dev_warn(&intf->dev, "cannot watch the status endpoint\n");
	dev_info(&intf->dev, "%s, card %d, %u frames x %u URBs\n",
		 card->longname, card->number, chip->base_urb_frames,
		 chip->nurbs);
	return 0;

free_card:
	if (chip->status_intf) {
		usb_driver_release_interface(&ffuc_driver, chip->status_intf);
		chip->status_intf = NULL;
	}
	snd_card_free(card);
release_idx:
	if (dev_idx >= 0) {
		mutex_lock(&devices_mutex);
		clear_bit(dev_idx, &devices_used);
		mutex_unlock(&devices_mutex);
	}
	return ret;
}

static void ffuc_disconnect(struct usb_interface *intf)
{
	struct ffuc *chip = usb_get_intfdata(intf);

	if (!chip)
		return;
	usb_set_intfdata(intf, NULL);

	snd_card_disconnect(chip->card);
	debugfs_remove_recursive(chip->debugfs);
	chip->debugfs = NULL;
	mutex_lock(&chip->mutex);
	WRITE_ONCE(chip->disconnected, true);
	WRITE_ONCE(chip->engine_running, false);
	ffuc_kill_urbs(chip, true);
	mutex_unlock(&chip->mutex);
	cancel_work_sync(&chip->recover_work);
	ffuc_hwdep_quiesce(chip);
	ffuc_midi_cleanup(chip);
	if (chip->status_intf) {
		usb_driver_release_interface(&ffuc_driver, chip->status_intf);
		chip->status_intf = NULL;
	}
	snd_card_free_when_closed(chip->card);
}

static int ffuc_suspend(struct usb_interface *intf, pm_message_t message)
{
	struct ffuc *chip = usb_get_intfdata(intf);
	unsigned int dir;

	if (!chip)
		return 0;

	chip->resume_mask = 0;
	for (dir = 0; dir < 2; dir++) {
		if (chip->streams[dir].running)
			chip->resume_mask |= 1u << dir;
	}
	dev_dbg(&chip->dev->dev,
		"suspend: prepared=0x%x running=0x%x\n",
		chip->prepared_mask, chip->resume_mask);

	snd_power_change_state(chip->card, SNDRV_CTL_POWER_D3hot);
	/*
	 * Only enter the ALSA suspended state when every prepared direction
	 * was also running.  A prepared but stopped PCM has no active stream
	 * to suspend, and marking it SUSPENDED makes some userspace graphs
	 * leave the sink suspended after resume and never restart it.  The
	 * driver stops the engine in all cases and restarts it on resume; a
	 * mixed running/prepared pair is left in its current ALSA state.
	 */
	if (chip->resume_mask && chip->resume_mask == chip->prepared_mask)
		snd_pcm_suspend_all(chip->pcm);
	else
		dev_dbg(&chip->dev->dev,
			"suspend: driver-managed (prepared=0x%x running=0x%x)\n",
			chip->prepared_mask, chip->resume_mask);
	cancel_work_sync(&chip->recover_work);
	mutex_lock(&chip->mutex);
	ffuc_engine_stop(chip);
	mutex_unlock(&chip->mutex);
	ffuc_midi_suspend(chip);
	ffuc_transport_suspend(chip);
	return 0;
}

static int ffuc_resume_common(struct usb_interface *intf, bool reset)
{
	struct ffuc *chip = usb_get_intfdata(intf);
	int ret, engine_ret = 0;

	if (!chip)
		return 0;

	if (reset) {
		mutex_lock(&chip->mutex);
		/*
		 * A USB reset loses the alternate setting and any UC session
		 * state.  Force the next engine start to select the alternate
		 * setting and, on the UC, replay the session-open sequence.
		 */
		if (chip->model == FFUC_MODEL_UC) {
			chip->uc_session = true;
			chip->session_reopen = true;
		}
		chip->cur_alt = 0;
		mutex_unlock(&chip->mutex);
	}

	mutex_lock(&chip->mutex);
	ret = ffuc_hw_setup(chip);
	if (ret == 0 && chip->prepared_mask && !chip->engine_running)
		engine_ret = ffuc_engine_start(chip);
	mutex_unlock(&chip->mutex);

	if (ret == 0) {
		int transport_ret = ffuc_transport_setup(chip);

		if (transport_ret < 0)
			dev_warn(&chip->dev->dev,
				 "resume: transport setup failed: %d\n",
				 transport_ret);
	}
	if (engine_ret < 0)
		dev_warn(&chip->dev->dev,
			 "resume: engine restart failed: %d\n", engine_ret);

	ffuc_midi_resume(chip);
	dev_dbg(&chip->dev->dev,
		"resume: reset=%d prepared=0x%x running=0x%x engine=%d ret=%d\n",
		reset, chip->prepared_mask, chip->resume_mask,
		READ_ONCE(chip->engine_running), ret);
	snd_power_change_state(chip->card, SNDRV_CTL_POWER_D0);
	return ret;
}

static int ffuc_resume(struct usb_interface *intf)
{
	return ffuc_resume_common(intf, false);
}

static int ffuc_reset_resume(struct usb_interface *intf)
{
	return ffuc_resume_common(intf, true);
}

static struct usb_driver ffuc_driver = {
	.name = "snd-fireface-uc",
	.probe = ffuc_probe,
	.disconnect = ffuc_disconnect,
	.suspend = ffuc_suspend,
	.resume = ffuc_resume,
	.reset_resume = ffuc_reset_resume,
	.id_table = ffuc_ids,
	.supports_autosuspend = 0,
};

module_usb_driver(ffuc_driver);

MODULE_DESCRIPTION("RME Fireface UC/UCX USB audio transport (PCM + hwdep)");
MODULE_LICENSE("GPL");
