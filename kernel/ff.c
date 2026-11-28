// SPDX-License-Identifier: GPL-2.0-only
/*
 * RME Fireface USB - kernel transport core.
 *
 * This file owns USB enumeration, the streaming engines, the ALSA PCM
 * device and the per-device descriptor table.  Device-model policy lives in
 * userspace behind the snd-hwdep node created by ff-hwdep.c; see
 * ff.h and docs/spec/DRIVER-DESIGN.md.
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

#include "ff.h"
#include "ff-gate.h"

static int index[SNDRV_CARDS] = SNDRV_DEFAULT_IDX;
static char *id[SNDRV_CARDS] = SNDRV_DEFAULT_STR;
static bool enable[SNDRV_CARDS] = SNDRV_DEFAULT_ENABLE_PNP;
static unsigned int urb_frames = 48;
static unsigned int iso_packets = 4;
static unsigned int nurbs = 2;
static bool ucx_if5;
static unsigned int ucx_if5_frames = 16;
static unsigned int uc_pc_frames = 16;
static unsigned int fixed_rate;
/* q48 instrumentation (test/uc-q48-instrumentation).  Poison the PCM rings
 * with 0x00005A5A instead of the inherited memory so that a reader's
 * preserved zero can be distinguished from never-written memory.  Values
 * are near-silent in both formats: float32 denormal and s32 DC null. */
static bool ff_poison = true;

static DEFINE_MUTEX(devices_mutex);
static unsigned long devices_used;

static struct usb_driver ff_driver;

static bool prefill = true;
module_param(prefill, bool, 0644);
MODULE_PARM_DESC(prefill, "Mute the playback outputs from trigger until the startup ring delivery is stable (default on).");

static bool debug;
module_param(debug, bool, 0644);
MODULE_PARM_DESC(debug, "Log ring-pressure events (underrun/overrun) at info level for local diagnosis (default off).");

static int lead_margin = -1;
module_param(lead_margin, int, 0644);
MODULE_PARM_DESC(lead_margin, "OUT lead bias override in frames; -1 = the reference bias 2*max_frames-2 (12/24/48 by band), the same on UC, UCX and Babyface-classic. Override to A/B the lead depth.");

module_param(ff_poison, bool, 0644);
MODULE_PARM_DESC(ff_poison, "Poison-fill the PCM rings with 0x00005A5A at prepare (diagnostics).");

module_param_array(index, int, NULL, 0444);
MODULE_PARM_DESC(index, "Index value for the Fireface sound card.");
module_param_array(id, charp, NULL, 0444);
MODULE_PARM_DESC(id, "ID string for the Fireface sound card.");
module_param_array(enable, bool, NULL, 0444);
MODULE_PARM_DESC(enable, "Enable the Fireface sound card.");
module_param(urb_frames, uint, 0444);
MODULE_PARM_DESC(urb_frames, "Interrupt transport: frames per URB at 32-48 kHz, multiple of 8 (8..256); doubled at 2x and quadrupled at 4x.");
module_param(iso_packets, uint, 0444);
MODULE_PARM_DESC(iso_packets, "ISO transport: microframes (USB packets) per URB (1..8, default 4 = 0.5 ms at every rate).");
module_param(nurbs, uint, 0444);
MODULE_PARM_DESC(nurbs, "URBs in flight per endpoint (2..32, default 2); raised so at least 64 frames stay queued, except on the ISO and UCX IF5 short-URB geometries.");
module_param(ucx_if5, bool, 0444);
MODULE_PARM_DESC(ucx_if5, "Fireface UCX: bind the Windows-style interrupt interface 5 (default false). The default binds the Mac-style ISO interface 0; set Y only for the Windows transport.");
module_param(ucx_if5_frames, uint, 0444);
MODULE_PARM_DESC(ucx_if5_frames, "Fireface UCX interrupt transport: frames per URB at 1x rates, doubled at 2x and quadrupled at 4x (8..1024, default 16 = 2 blocks of 8).");
module_param(uc_pc_frames, uint, 0444);
MODULE_PARM_DESC(uc_pc_frames, "Fireface UC PC interrupt transport: frames per URB at 1x rates, doubled at 2x and quadrupled at 4x (8..1024, default 16 = 2 blocks of 8).");
module_param(fixed_rate, uint, 0644);
MODULE_PARM_DESC(fixed_rate, "Only allow this sample rate (0 = any supported rate).");

/* -- sample rates and bandwidth classes ----------------------------- */

static const struct ff_mode ff_pc_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .pkt_frames = 8 },	/* 1x */
	{ .alt = 2, .neps = 1, .words = 14, .pkt_frames = 16 },	/* 2x */
	{ .alt = 3, .neps = 2, .words = 6, .pkt_frames = 32 },	/* 4x */
};

/* UCX interrupt transport (interface 5).  The Windows reference host
 * selects alt 1 once at connect and streams 44.1/48 kHz on it (E2b/E5,
 * win-20261027-ucx-transport).  Alt 2 carries 2x (14 words, ADAT SMUX);
 * alt 3 is the 4x path with two capture endpoints.  The 2x/4x alternate
 * selection awaits a capture. */
static const struct ff_mode ff_ucx_if5_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .pkt_frames = 8 },
	{ .alt = 2, .neps = 1, .words = 14, .pkt_frames = 16 },
	{ .alt = 3, .neps = 2, .words = 6, .pkt_frames = 32 },
};

/* ISO transport (UC Mac personality and UCX interface 0).  The packet
 * maxima are wMaxPacketSize * mult from the descriptors; the UCX OUT
 * packet adds one lead word per frame. */
static const struct ff_mode ff_mac_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .in_mps = 508, .out_mps = 532, .max_frames = 7 },
	{ .alt = 2, .neps = 1, .words = 14, .in_mps = 732, .out_mps = 780, .max_frames = 13 },
	{ .alt = 3, .neps = 1, .words = 12, .in_mps = 1208, .out_mps = 1304, .max_frames = 25 },
};

static const struct ff_mode ff_ucx_modes[] = {
	{ .alt = 1, .neps = 1, .words = 18, .in_mps = 508, .out_mps = 532, .max_frames = 7 },
	{ .alt = 2, .neps = 1, .words = 14, .in_mps = 732, .out_mps = 780, .max_frames = 13 },
	{ .alt = 3, .neps = 1, .words = 12, .in_mps = 1208, .out_mps = 1304, .max_frames = 25 },
};

/* Babyface-classic ISO interface 0.  Same packet format as the UCX; the
 * counts follow from wMaxPacketSize: IN = 4 + frames * words * 4,
 * OUT = frames * (1 + out_words) * 4. */
static const struct ff_mode ff_babyface_classic_modes[] = {
	{ .alt = 1, .neps = 1, .words = 12, .out_words = 14, .in_mps = 340, .out_mps = 420, .max_frames = 7 },
	{ .alt = 2, .neps = 1, .words = 8, .out_words = 10, .in_mps = 420, .out_mps = 572, .max_frames = 13 },
	{ .alt = 3, .neps = 1, .words = 6, .out_words = 8, .in_mps = 604, .out_mps = 900, .max_frames = 25 },
};

static const unsigned int ff_rates[] = {
	32000, 44100, 48000, 64000, 88200, 96000, 128000, 176400, 192000,
	352800, 384000, 705600, 768000,
};

static const struct snd_pcm_hw_constraint_list ff_rate_list = {
	.count = ARRAY_SIZE(ff_rates),
	.list = ff_rates,
};

static unsigned int ff_mode_index(unsigned int rate)
{
	if (rate <= 48000)
		return 0;
	if (rate <= 96000)
		return 1;
	return 2;
}

static bool ff_rate_supported(struct ff *chip, unsigned int rate)
{
	int i;

	if (rate > chip->desc->max_rate)
		return false;
	for (i = 0; i < ARRAY_SIZE(ff_rates); i++)
		if (ff_rates[i] == rate)
			return true;
	return false;
}

static unsigned int ff_mode_channels(struct ff *chip, unsigned int rate)
{
	const struct ff_mode *m = &chip->modes[ff_mode_index(rate)];

	/* Playback words: the stream open/close words mute and unmute
	 * playback channels. */
	return m->neps * ff_mode_out_words(m);
}

/*
 * The ALSA PCM keeps a constant logical channel count.  At 2x/4x the USB
 * packet carries fewer words (ADAT S/MUX); the engine maps the first
 * 14/12 words and the remaining channels stay silent.  This lets PipeWire
 * keep one 18-channel node while the card runs at every supported rate.
 */
static unsigned int ff_max_channels(struct ff *chip, int stream,
				      unsigned int rate)
{
	(void)rate;
	return stream == SNDRV_PCM_STREAM_PLAYBACK ?
	       ff_desc_playback(chip->desc) : chip->desc->max_channels;
}

static unsigned int ff_urb_frames(struct ff *chip, unsigned int rate)
{
	if (chip->iso)
		return chip->iso_packets * DIV_ROUND_UP(rate, 8000);
	/* The interrupt URB is a fixed number of frames at 1x and doubles or
	 * quadruples with the rate family, like the reference UCX engine: a
	 * block is 8/16/32 frames and a URB is a fixed number of blocks. */
	return chip->base_urb_frames << ff_mode_index(rate);
}

/* The feedback read offset: three URBs, floored so a one- or two-packet
 * URB cannot place the OUT fill ahead of the IN feedback.
 */
static unsigned int ff_iso_fb_delay(const struct ff *chip)
{
	return max((chip->nurbs + 1) * chip->iso_packets,
		   (unsigned int)FF_MIN_FB_PACKETS);
}

/* AP-mode ISO header layout per rate band (1x/2x/4x).  The position field
 * widens with the band (bits 5..17/18/19); the phase field is FIXED at
 * bits 21..31 (11-bit) at every band, above the widened position.  The
 * layout is generic across the IF0 models (UC, UCX, Babyface-classic share it);
 * only the rate band selects the position width.  The phase VALUE wraps at
 * tps = 60000000/rate (a 60 MHz tick), so the fractional-position law
 * phase/tps is band-independent. */
struct ff_iso_layout {
	u32 pos_mask;
	u32 modulus;
	unsigned int phase_shift;
	u32 phase_mask;
};

static const struct ff_iso_layout ff_iso_layouts[3] = {
	{ 0x1fff, 0x2000, 21, 0x7ff },	/* 1x: pos 5..17, phase 21..31 */
	{ 0x3fff, 0x4000, 21, 0x7ff },	/* 2x: pos 5..18, phase 21..31 */
	{ 0x7fff, 0x8000, 21, 0x7ff },	/* 4x: pos 5..19, phase 21..31 */
};

static const struct ff_iso_layout *ff_iso_layout(struct ff *chip)
{
	return &ff_iso_layouts[ff_mode_index(chip->rate)];
}

static u32 ff_iso_pos_mask(struct ff *chip)
{
	return ff_iso_layout(chip)->pos_mask;
}

static unsigned int ff_iso_phase_shift(struct ff *chip)
{
	return ff_iso_layout(chip)->phase_shift;
}

static u32 ff_iso_phase_mask(struct ff *chip)
{
	return ff_iso_layout(chip)->phase_mask;
}

unsigned int ff_iso_wrap(struct ff *chip)
{
	/* The position counter modulus widens with the rate band: 8192 at
	 * 1x, 16384 at 2x, 32768 at 4x.  It is a power of two, so the field
	 * mask and the modulo reduction agree. */
	return ff_iso_layout(chip)->modulus;
}

/* The OUT lead word: the feedback offset plus the reference bias.  The
 * lead_margin parameter overrides the bias for A/B; the steady word is
 * re-aligned once at engine start (ff_iso_align).
 */
static unsigned int ff_iso_out_lead(struct ff *chip)
{
	const struct ff_mode *m = chip->mode;
	int margin = lead_margin >= 0 ? lead_margin : (2 * m->max_frames - 2);

	return (DIV_ROUND_CLOSEST(ff_iso_fb_delay(chip) * chip->rate, 8000) +
		margin) % ff_iso_wrap(chip);
}

unsigned int ff_rate_from_index(unsigned int idx)
{
	static const unsigned int map[12] = {
		32000, 44100, 48000, 48000, 64000, 88200,
		96000, 96000, 128000, 176400, 192000, 192000,
	};

	return idx < ARRAY_SIZE(map) ? map[idx] : 0;
}

/* -- vendor requests ------------------------------------------------- */

static void ff_track_clock(struct ff *chip, u16 val, u16 idx);
static unsigned int ff_programmed_clock(const struct ff *chip);

int ff_vendor_send(struct ff *chip, u8 req, u16 val, u16 idx,
		     const void *data, u16 len)
{
	int ret;

	if (READ_ONCE(chip->disconnected))
		return -ENODEV;
	ret = usb_control_msg_send(chip->dev, 0, req,
				   USB_DIR_OUT | USB_TYPE_VENDOR |
				   USB_RECIP_DEVICE, val, idx,
				   (void *)data, len, FF_CTL_TIMEOUT,
				   GFP_KERNEL);
	if (ret >= 0) {
		if (req == FF_REQ_DDS)
			ff_track_clock(chip, val, idx);
		else if (req == FF_REQ_SETTINGS && idx == FF_OPTIONS_MASK)
			atomic_inc(&chip->option_writes);
	}
	return ret;
}

int ff_vendor_recv(struct ff *chip, u8 req, u16 val, u16 idx,
		     void *data, u16 len)
{
	if (READ_ONCE(chip->disconnected))
		return -ENODEV;
	return usb_control_msg_recv(chip->dev, 0, req,
				    USB_DIR_IN | USB_TYPE_VENDOR |
				    USB_RECIP_DEVICE, val, idx,
				    data, len, FF_CTL_TIMEOUT, GFP_KERNEL);
}

/* One IF1 register word: [parity:1][addr:15][value:16], little-endian. */
static u32 ff_word_encode(u16 addr, u16 value)
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

int ff_reg_send(struct ff *chip, const struct ff_hwdep_word *words,
		  unsigned int count)
{
	u8 *buf;
	unsigned int done = 0;
	int ret = 0;

	if (!chip->desc->state_out_ep)
		return -ENODEV;
	if (!count)
		return 0;
	if (count > FF_HWDEP_MAX_WORDS)
		count = FF_HWDEP_MAX_WORDS;

	buf = kmalloc(count * sizeof(u32), GFP_KERNEL);
	if (!buf)
		return -ENOMEM;

	while (done < count) {
		unsigned int n = min_t(unsigned int, count - done,
				       FF_HWDEP_MAX_WORDS);
		unsigned int i, actual;
		u8 *p = buf;

		for (i = 0; i < n; i++) {
			u32 w = ff_word_encode(words[done + i].addr,
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
 * interface must be claimed explicitly.  The helpers keep ff_driver local
 * to this file while ff-hwdep.c can still manage the interface. */
int ff_claim_interface(struct usb_interface *intf)
{
	return usb_driver_claim_interface(&ff_driver, intf, NULL);
}

void ff_release_interface(struct usb_interface *intf)
{
	usb_driver_release_interface(&ff_driver, intf);
}

/* -- per-device core sequences --------------------------------------- */

/*
 * UC channel initialization.  The 0x16 wValue is the per-channel sample
 * format: 0 = S32, 1 = float32.  It must equal the wire format the host
 * sends (ff_wire_s32); a mismatch passes only the sample sign bit and
 * distorts the audio.  The value is therefore derived from the resolved
 * wire format, not stored per range.  Both UC personalities use the
 * 36-slot init (18 inputs + 18 outputs); the interrupt personality pairs
 * it with S32 and the ISO personality with float32.  The Windows host
 * instead writes a 60-slot block with S32 (documented in the protocol
 * spec); the slot count does not affect the format pairing.
 */
static const struct ff_init_range ff_uc_chan_init[] = {
	{ .first = 0,    .count = 18 },
	{ .first = 0x20, .count = 18 },
};

static const struct ff_init_range ff_ucx_chan_init[] = {
	{ .first = 0,    .count = FF_MAX_CHANNELS },
	{ .first = 0x20, .count = FF_MAX_CHANNELS },
};

/* UCX interrupt init covers 60 slots: 0..29 and 32..61 (E2b capture). */
static const struct ff_init_range ff_ucx_if5_chan_init[] = {
	{ .first = 0,    .count = 30 },
	{ .first = 0x20, .count = 30 },
};

/* Write the descriptor's 0x16 slot ranges.  The wValue is the channel
 * sample format and follows the resolved wire format (0 = S32,
 * 1 = float32). */
static int ff_uc_chan_init_write(struct ff *chip)
{
	const struct ff_init_range *range;
	u16 value = chip->s32_samples ? 0x0000 : 0x0001;
	unsigned int i, r;
	int ret;

	for (r = 0; r < chip->desc->n_chan_init; r++) {
		range = &chip->desc->chan_init[r];
		for (i = 0; i < range->count; i++) {
			ret = ff_vendor_send(chip, FF_REQ_CHAN_INIT,
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
static int ff_uc_init(struct ff *chip)
{
	int ret;

	ret = ff_uc_chan_init_write(chip);
	if (ret < 0)
		return ret;
	ret = ff_vendor_send(chip, FF_REQ_SETTINGS, 0x0800, 0xb800,
			       NULL, 0);
	if (ret < 0)
		return ret;
	return ff_vendor_send(chip, FF_REQ_ENABLE, 0x0001, 0x0000,
				NULL, 0);
}

unsigned int ff_rate_family(unsigned int rate)
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

/* The host shifts the device clock with the DDS divisor (varispeed). The
 * active family's divisor encodes base/(1 + pitch/100); the band multiplier
 * follows the streaming rate. Recover the programmed clock so the cadence
 * judge compares the measured clock against the actual clock, not the
 * nominal rate. The full-rate division keeps the truncation under 1 Hz,
 * far inside the judge's ±1% band. Only the active family's word moves the
 * current clock; the other three families are stored for later rates. */
static void ff_track_clock(struct ff *chip, u16 val, u16 idx)
{
	unsigned int fam = idx & 0x03;

	/* The ISO path remembers the host's word for this family; a start
	 * re-sends it, so varispeed survives an engine restart.  The packed
	 * word is stored before the valid bit is set.  The interrupt path
	 * keeps the Windows rate sequence and does not use this store. */
	if (chip->iso) {
		WRITE_ONCE(chip->dds_word[fam], ((u32)val << 16) | idx);
		set_bit(fam, &chip->dds_valid);
	}
	if (!chip->rate)
		return;
	if (fam != ff_rate_family(chip->rate))
		return;
	WRITE_ONCE(chip->clock_rate, ff_dds_clock(val, idx, chip->rate));
}

static int ff_set_rate_family(struct ff *chip, unsigned int rate)
{
	return ff_vendor_send(chip, FF_REQ_SETTINGS,
				ff_rate_family(rate) << 4, 0x0030, NULL, 0);
}

/* The rate sequence has one form per transport.
 *
 * ISO follows the USB audio engine's hwSetSampleRate: one DDS divisor word
 * for the target base family, then the family selector at 0x10/0x30.  Both
 * words are written on every start, change and resume.  The DDS word is the
 * actual clock; the family selector only chooses which stored divisor is
 * active.  The options word is not part of the sequence: settings writes own
 * it.  The host owns varispeed; the start re-sends the family's last written
 * word, so a pitched clock survives a restart.  An absent family falls back
 * to its nominal divisor.
 *
 * Interrupt keeps the Windows-derived sequence: the nominal DDS word, the
 * family selector, then the options commit.  The kext analysis does not
 * change this path; it needs the Windows binary or a VM capture. */
static int ff_set_rate_iso(struct ff *chip, unsigned int rate)
{
	unsigned int fam = ff_rate_family(rate);
	u16 val, idx;
	u32 dds;
	int ret;

	if (test_bit(fam, &chip->dds_valid)) {
		u32 word = READ_ONCE(chip->dds_word[fam]);
		u32 v = word >> 16, i = word & 0xffff;

		/* The host wrote this word at some band.  The target band's
		 * packet capacity is the hard ceiling, so clamp the reused
		 * word before it reaches the device. */
		if (ff_dds_clamp(v, i, rate,
				   chip->modes[ff_mode_index(rate)].max_frames,
				   &v, &i))
			atomic_inc(&chip->pitch_clamps);
		val = v;
		idx = i;
	} else {
		dds = (u32)div_u64(0x5f5e10000000ULL, rate);
		if (rate > 114000)
			dds <<= 2;
		else if (rate > 57000)
			dds <<= 1;
		val = dds >> 16;
		idx = (dds & 0xff00) | fam;
	}

	ret = ff_vendor_send(chip, FF_REQ_DDS, val, idx, NULL, 0);
	if (ret < 0)
		return ret;
	atomic_inc(&chip->dds_words);
	ret = ff_set_rate_family(chip, rate);
	if (ret < 0)
		return ret;
	atomic_inc(&chip->family_writes);
	dev_dbg(&chip->dev->dev,
		"rate %u Hz (iso): dds 0x%04x/0x%04x family 0x%02x ret=%d\n",
		rate, val, idx, fam << 4, ret);
	return 0;
}

static int ff_set_rate_int(struct ff *chip, unsigned int rate)
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

	ret = ff_vendor_send(chip, FF_REQ_DDS, dds >> 16,
			       (dds & 0xff00) | ff_rate_family(rate),
			       NULL, 0);
	if (ret < 0)
		return ret;
	atomic_inc(&chip->dds_words);
	ret = ff_set_rate_family(chip, rate);
	if (ret < 0)
		return ret;
	atomic_inc(&chip->family_writes);
	ret = ff_vendor_recv(chip, FF_REQ_SETTINGS, 0, 0, &cur,
			       sizeof(cur));
	if (ret < 0)
		return ret;
	opts = le32_to_cpu(cur) & FF_OPTIONS_MASK;
	ret = ff_vendor_send(chip, FF_REQ_SETTINGS, opts,
			       FF_OPTIONS_MASK, NULL, 0);
	dev_dbg(&chip->dev->dev,
		"rate %u Hz (int): dds 0x%04x/0x%04x family 0x%02x commit 0x%04x ret=%d\n",
		rate, dds >> 16, (dds & 0xff00) | ff_rate_family(rate),
		ff_rate_family(rate) << 4, opts, ret);
	return ret;
}

/* The UCX interrupt rate change mirrors the Windows capture: it writes the
 * family selector only.  The four-word DDS quad is written at connect
 * (ff_ucx_if5_init) and by the host on a settings apply.  The DDS words are
 * not rewritten on a rate change. */
static int ff_set_rate_int_ucx(struct ff *chip, unsigned int rate)
{
	int ret;

	ret = ff_set_rate_family(chip, rate);
	if (ret < 0)
		return ret;
	atomic_inc(&chip->family_writes);
	dev_dbg(&chip->dev->dev,
		"rate %u Hz (int ucx): family 0x%02x ret=%d\n",
		rate, ff_rate_family(rate) << 4, ret);
	return 0;
}

static int ff_set_rate(struct ff *chip, unsigned int rate)
{
	return chip->iso ? ff_set_rate_iso(chip, rate) :
			   ff_set_rate_int(chip, rate);
}

static int ff_uc_hw_mute(struct ff *chip, bool mute)
{
	return ff_vendor_send(chip,
				mute ? FF_REQ_MUTE : FF_REQ_UNMUTE,
				0xffff, 0xc000, NULL, 0);
}

/* Requests 19/20 mute/unmute playback channels by mask.  UC/UCX: channel
 * n is bit n + 2, high half in wValue, low half in wIndex.
 * Babyface-classic: see below. */
static int ff_playback_mask(struct ff *chip, bool mute,
			      unsigned int channels)
{
	u32 mask;
	u16 idx, val;

	if (chip->caps & FF_TCAP_MUTE_MASK_18BIT) {
		/* The Babyface-classic reads an 18-bit mask packed like a
		 * 0x12 level: channel n is bit n, wValue = mask >> 2,
		 * wIndex bits 14-15 = mask bits 0-1.  The vendor driver
		 * sends all 18 bits. */
		mask = (1u << FF_MAX_CHANNELS) - 1;
		val = mask >> 2;
		idx = (mask & 3) << 14;
	} else {
		if (channels > 30)
			channels = 30;
		mask = ((1u << channels) - 1) << 2;
		val = mask >> 16;
		idx = mask & 0xffff;
	}
	return ff_vendor_send(chip,
				mute ? FF_REQ_UCX_MUTE :
				       FF_REQ_UCX_UNMUTE,
				val, idx, NULL, 0);
}

static int ff_ucx_hw_mute(struct ff *chip, bool mute)
{
	return ff_playback_mask(chip, mute, FF_MAX_CHANNELS);
}

static int ff_ucx_stream_start(struct ff *chip, unsigned int channels)
{
	int ret;

	/* The Windows driver sends these immediately before the first
	 * audio packet.  0x8000/0x8000 is required for the DACs to open on
	 * the interrupt transport; 0x1d is not understood but is part of the
	 * captured working sequence. */
	ret = ff_vendor_send(chip, FF_REQ_SETTINGS, 0x8000, 0x8000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ff_vendor_send(chip, FF_REQ_UCX_STREAM29, 0x0000, 0x0000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	return ff_playback_mask(chip, false, channels);
}

static int ff_ucx_stream_stop(struct ff *chip, unsigned int channels)
{
	return ff_playback_mask(chip, true, channels);
}

static int ff_uc_stream_start(struct ff *chip, unsigned int channels)
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
	ret = ff_vendor_send(chip, FF_REQ_SETTINGS, 0x0000, 0x8000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ff_vendor_send(chip, FF_REQ_UCX_STREAM29, 0x0000, 0x0000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ff_vendor_send(chip, FF_REQ_UNMUTE, 0x000f, 0xc000,
			       NULL, 0);
	if (ret < 0)
		return ret;
	chip->session_reopen = false;
	return 0;
}

static int ff_uc_stream_stop(struct ff *chip, unsigned int channels)
{
	(void)channels;
	/* Windows closes the session before every rate write (A2 t=13.03,
	 * rate at 13.25; FP9c t=4.88, rate at 10.48).  This is not gated on
	 * a previous rate change: the first 48 -> 44.1 switch needs it. */
	dev_dbg(&chip->dev->dev,
		"UC session close (rate=%u channels=%u)\n",
		chip->rate, channels);
	/* Windows session-close word: 0x13 wIndex 0xC000 wValue 0x000F. */
	return ff_vendor_send(chip, FF_REQ_MUTE, 0x000f, 0xc000,
				NULL, 0);
}

static const struct ff_ops ff_uc_ops = {
	.init = ff_uc_init,
	.set_rate = ff_set_rate,
	.stream_start = ff_uc_stream_start,
	.stream_stop = ff_uc_stream_stop,
	.hw_mute = ff_uc_hw_mute,
};

static const struct ff_ops ff_ucx_ops = {
	.init = ff_uc_init,
	.set_rate = ff_set_rate,
	.stream_start = ff_ucx_stream_start,
	.stream_stop = ff_ucx_stream_stop,
	.hw_mute = ff_ucx_hw_mute,
};

/* UCX interrupt (IF5) connect init, captured from the Windows driver
 * (E2b replug, win-20261027-ucx-transport): the 60-slot channel init,
 * the DDS/clock quad, the gain-ref word and the settings group.  The
 * quad is written once at connect; a rate change writes the family word
 * only.  The settings group carries a ~100 ms settle between the 0x3000
 * writes, measured in the capture. */
static int ff_ucx_if5_init(struct ff *chip)
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

	ret = ff_uc_chan_init_write(chip);
	if (ret < 0)
		return ret;
	for (i = 0; i < 4; i++) {
		ret = ff_vendor_send(chip, FF_REQ_DDS, dds[i][0],
				       dds[i][1], NULL, 0);
		if (ret < 0)
			return ret;
	}
	ret = ff_vendor_send(chip, FF_REQ_SETTINGS, 0x0028, 0x05ff,
			       NULL, 0);
	if (ret < 0)
		return ret;
	ret = ff_vendor_send(chip, FF_REQ_LEVELS, 0x3c00, 0x0fc0,
			       NULL, 0);
	if (ret < 0)
		return ret;
	for (i = 0; i < 5; i++) {
		ret = ff_vendor_send(chip, FF_REQ_SETTINGS, grp_pre[i][0],
				       grp_pre[i][1], NULL, 0);
		if (ret < 0)
			return ret;
	}
	msleep(FF_UCX_IF5_SETTLE_MS);
	for (i = 0; i < 6; i++) {
		ret = ff_vendor_send(chip, FF_REQ_SETTINGS,
				       grp_post[i][0], grp_post[i][1],
				       NULL, 0);
		if (ret < 0)
			return ret;
	}
	return 0;
}

static const struct ff_ops ff_ucx_if5_ops = {
	.init = ff_ucx_if5_init,
	.set_rate = ff_set_rate_int_ucx,
	.stream_start = ff_ucx_stream_start,
	.stream_stop = ff_ucx_stream_stop,
	.hw_mute = ff_ucx_hw_mute,
};

/* Resolve the wire sample format once at probe.  The format is keyed to
 * the transport: the interrupt personalities carry S32 (the Windows
 * reference pairing) and the ISO personalities carry float32 (the macOS
 * reference pairing).  The device accepts either format when the 0x16
 * channel-init value matches; ff_uc_chan_init_write derives that value
 * from this decision so the two can never mismatch. */
static bool ff_wire_s32(const struct ff *chip)
{
	return !chip->iso;
}

/* -- per-device descriptors ------------------------------------------ */

/* Named indices into ff_devices[].  The USB ID table references these
 * instead of raw positions, and the count assertion after the table fails
 * the build when a descriptor is added without updating the enum.  Append
 * new devices at the end of both. */
enum {
	FF_DEV_UC_PC,
	FF_DEV_UC_MAC_SMSC,
	FF_DEV_UC_MAC_RME,
	FF_DEV_UCX_ISO_SMSC,
	FF_DEV_UCX_IF5_SMSC,
	FF_DEV_UCX_ISO_RME,
	FF_DEV_UCX_IF5_RME,
	FF_DEV_BABYFACE_CLASSIC,
	FF_DEV_UC_WIN_RME,
	FF_DEV_COUNT
};

static const struct ff_device_desc ff_devices[] = {
	{
		.vendor = FF_VENDOR_SMSC,
		.product = FF_PRODUCT_ID_A,
		.iface = 0,
		.transport = FF_XPORT_INTERRUPT,
		.quirks = FF_QUIRK_STATUS_LATCH | FF_QUIRK_WIN_SESSION,
		.modes = ff_pc_modes,
		.n_modes = ARRAY_SIZE(ff_pc_modes),
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
		.chan_init = ff_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_uc_chan_init),
		.ops = &ff_uc_ops,
	}, {
		.vendor = FF_VENDOR_SMSC,
		.product = FF_PRODUCT_ID_C,
		.iface = 0,
		.transport = FF_XPORT_ISO,
		.quirks = 0,
		.modes = ff_mac_modes,
		.n_modes = ARRAY_SIZE(ff_mac_modes),
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
		.chan_init = ff_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_uc_chan_init),
		.ops = &ff_uc_ops,
	}, {
		.vendor = FF_VENDOR_RME,
		.product = FF_PRODUCT_ID_C,
		.iface = 0,
		.transport = FF_XPORT_ISO,
		.quirks = 0,
		.modes = ff_mac_modes,
		.n_modes = ARRAY_SIZE(ff_mac_modes),
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
		.chan_init = ff_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_uc_chan_init),
		.ops = &ff_uc_ops,
	}, {
		.vendor = FF_VENDOR_SMSC,
		.product = FF_PRODUCT_ID_D,
		.iface = 0,
		.transport = FF_XPORT_ISO,
		.quirks = 0,
		.modes = ff_ucx_modes,
		.n_modes = ARRAY_SIZE(ff_ucx_modes),
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
		.chan_init = ff_ucx_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_ucx_chan_init),
		.ops = &ff_ucx_ops,
	}, {
		.vendor = FF_VENDOR_SMSC,
		.product = FF_PRODUCT_ID_D,
		.iface = 5,
		.transport = FF_XPORT_INTERRUPT,
		.quirks = FF_QUIRK_ALT_RESELECT |
			  FF_QUIRK_IF5_PRE_START |
			  FF_QUIRK_IF5_PLAYBACK_MASK,
		.modes = ff_ucx_if5_modes,
		.n_modes = ARRAY_SIZE(ff_ucx_if5_modes),
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
		.chan_init = ff_ucx_if5_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_ucx_if5_chan_init),
		.ops = &ff_ucx_if5_ops,
	}, {
		.vendor = FF_VENDOR_RME,
		.product = FF_PRODUCT_ID_D,
		.iface = 0,
		.transport = FF_XPORT_ISO,
		.quirks = 0,
		.modes = ff_ucx_modes,
		.n_modes = ARRAY_SIZE(ff_ucx_modes),
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
		.chan_init = ff_ucx_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_ucx_chan_init),
		.ops = &ff_ucx_ops,
	}, {
		.vendor = FF_VENDOR_RME,
		.product = FF_PRODUCT_ID_D,
		.iface = 5,
		.transport = FF_XPORT_INTERRUPT,
		.quirks = FF_QUIRK_ALT_RESELECT |
			  FF_QUIRK_IF5_PRE_START |
			  FF_QUIRK_IF5_PLAYBACK_MASK,
		.modes = ff_ucx_if5_modes,
		.n_modes = ARRAY_SIZE(ff_ucx_if5_modes),
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
		.chan_init = ff_ucx_if5_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_ucx_if5_chan_init),
		.ops = &ff_ucx_if5_ops,
	}, {
		/* ISO audio, MIDI and the IF1 0x85 DSP meter read-back.  The
		 * IF1 bulk OUT 0x0a carries EQ blocks and is not exposed yet;
		 * the roles of interfaces 3 (0x09/0x88) and 4 (0x8b) are not
		 * mapped.  The meter packet length is not known, so the URB
		 * takes a full 512-byte bulk packet and passes the actual
		 * length on. */
		.vendor = FF_VENDOR_RME,
		.product = FF_PRODUCT_ID_E,
		.iface = 0,
		.transport = FF_XPORT_ISO,
		.quirks = 0,
		.modes = ff_babyface_classic_modes,
		.n_modes = ARRAY_SIZE(ff_babyface_classic_modes),
		.audio_iface = 0,
		.out_ep = 0x03,
		.in_ep = 0x84,
		.status_iface = FF_IFACE_NONE,
		.state_iface = 1,
		.meter_in_ep = 0x85,
		.meter_len = 512,
		.midi_iface = 2,
		.midi_out_ep = 0x07,
		.midi_in_ep = 0x86,
		.max_channels = 12,
		.max_playback = 14,
		.max_rate = 192000,
		.ops = &ff_ucx_ops,
	}, {
		.vendor = FF_VENDOR_RME,
		.product = FF_PRODUCT_ID_B,
		.iface = 0,
		.transport = FF_XPORT_INTERRUPT,
		/* The output latch clears only on an alt 0 -> alt N reselect,
		 * as on the UCX.  The wire format follows the transport rule
		 * (S32 on the interrupt personality). */
		.quirks = FF_QUIRK_STATUS_LATCH |
			  FF_QUIRK_ALT_RESELECT |
			  FF_QUIRK_WIN_SESSION,
		.modes = ff_pc_modes,
		.n_modes = ARRAY_SIZE(ff_pc_modes),
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
		.chan_init = ff_uc_chan_init,
		.n_chan_init = ARRAY_SIZE(ff_uc_chan_init),
		.ops = &ff_uc_ops,
	},
};

/* The ID table indexes ff_devices by the named enum above; a new
 * descriptor without a new enum entry (or vice versa) fails the build. */
static_assert(ARRAY_SIZE(ff_devices) == FF_DEV_COUNT);

static const struct usb_device_id ff_ids[] = {
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_SMSC, FF_PRODUCT_ID_A, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UC_PC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_RME, FF_PRODUCT_ID_A_RME, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UC_PC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_RME, FF_PRODUCT_ID_B, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UC_WIN_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_SMSC, FF_PRODUCT_ID_C, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UC_MAC_SMSC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_RME, FF_PRODUCT_ID_C, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UC_MAC_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_SMSC, FF_PRODUCT_ID_D, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UCX_ISO_SMSC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_SMSC, FF_PRODUCT_ID_D, 5),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UCX_IF5_SMSC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_RME, FF_PRODUCT_ID_D, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UCX_ISO_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_RME, FF_PRODUCT_ID_D, 5),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_UCX_IF5_RME] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_SMSC, FF_PRODUCT_ID_E, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_BABYFACE_CLASSIC] },
	{ USB_DEVICE_INTERFACE_NUMBER(FF_VENDOR_RME, FF_PRODUCT_ID_E, 0),
	  .driver_info = (kernel_ulong_t)&ff_devices[FF_DEV_BABYFACE_CLASSIC] },
	{ }
};
MODULE_DEVICE_TABLE(usb, ff_ids);

/* -- sample format conversion --------------------------------------- */

static u32 ff_s32_to_f32(s32 v)
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

static s32 ff_f32_to_s32(u32 f)
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
static void ff_ring_event(struct ff *chip, struct snd_pcm_substream *subs,
			    atomic_t *counter, const char *what,
			    unsigned long appl, unsigned long own)
{
	atomic_inc(counter);
	if (debug)
		dev_info_ratelimited(&chip->dev->dev,
				     "%s: %s #%d appl %lu hw %lu (buffer %lu)\n",
				     subs->stream ? "capture" : "playback", what,
				     atomic_read(counter), appl, own,
				     subs->runtime->buffer_size);
	else
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
 * clamps the fill to zero and sets FF_RING_F_BEHIND. */
static void ff_ring_trace(struct ff *chip, struct snd_pcm_substream *subs,
			    unsigned long read_pos, unsigned long appl,
			    unsigned long status_hw, unsigned int total,
			    bool measure_stall)
{
	struct snd_pcm_runtime *rt = subs->runtime;
	unsigned long span = (appl + rt->boundary - read_pos) % rt->boundary;
	unsigned long pspan = (appl + rt->boundary - status_hw) % rt->boundary;
	unsigned long filled = 0, pub = 0;
	u64 now = ktime_get_ns();
	u32 flags = 0;
	struct ff_ring_rec *r;

	/* The true difference is within a few buffers of zero, far below
	 * boundary/2: a span above boundary/2 is a negative difference. */
	if (span <= rt->boundary / 2)
		filled = span;
	else
		flags |= FF_RING_F_BEHIND;
	if (pspan <= rt->boundary / 2)
		pub = pspan;
	if (total > filled)
		flags |= FF_RING_F_STARVED;
	if (total > pub)
		flags |= FF_RING_F_UNDERRUN;
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
				flags |= FF_RING_F_MISSED;
				atomic_inc(&chip->ring_missed);
			}
		} else {
			u64 gap = now - chip->ring_last_appl_ts;

			if (measure_stall && gap > chip->ring_max_stall_ns)
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
	if (flags & FF_RING_F_STARVED)
		atomic_inc(&chip->ring_starved);

	r = &chip->ring_trace[chip->ring_head % FF_RING_TRACE];
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
static void ff_ptr_mismatch(struct ff *chip,
			      struct snd_pcm_substream *subs,
			      const struct ff_stream *s)
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
static void ff_capture_copy(struct snd_pcm_runtime *rt, snd_pcm_uframes_t pos,
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

			dst[coff + c] = is_float ? v : (u32)ff_f32_to_s32(v);
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

static void ff_capture_copy_int32(struct snd_pcm_runtime *rt,
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

			dst[coff + c] = is_float ? ff_s32_to_f32((s32)v) : v;
		}
		for (; c + coff < chans; c++)
			if (coff + c >= total)
				dst[coff + c] = 0;
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static void ff_playback_copy(struct snd_pcm_runtime *rt, snd_pcm_uframes_t pos,
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
				    ff_s32_to_f32((s32)src[coff + c]);
			dst[c] = cpu_to_le32(v);
		}
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static void ff_playback_copy_int32(struct snd_pcm_runtime *rt,
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
				    (u32)ff_f32_to_s32(src[coff + c]) :
				    src[coff + c];
			dst[c] = cpu_to_le32(v);
		}
		if (++p == rt->buffer_size)
			p = 0;
	}
}

static bool ff_stream_advance(struct ff *chip, struct ff_stream *s,
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
static bool ff_pending(struct ff_stream *s)
{
	return s->running && s->period != s->signalled;
}

static void ff_signal_period(struct ff *chip, struct ff_stream *s)
{
	unsigned long flags;

	if (!s->substream)
		return;
	snd_pcm_stream_lock_irqsave(s->substream, flags);
	if (!ff_pending(s)) {
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
			ff_ptr_mismatch(chip, s->substream, s);
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
static void ff_period_elapsed(struct ff *chip, int dir)
{
	struct ff_stream *p = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	struct ff_stream *c = &chip->streams[SNDRV_PCM_STREAM_CAPTURE];
	unsigned long flags;

	/* The two directions' completions can run concurrently on
	 * different CPUs; the gate decision and its deferral counters
	 * form one serialized section. Lock order: gate_lock, then the
	 * stream lock inside ff_signal_period.
	 */
	spin_lock_irqsave(&chip->gate_lock, flags);
	if (!p->running || !c->running) {
		ff_signal_period(chip, p->running ? p : c);
		spin_unlock_irqrestore(&chip->gate_lock, flags);
		return;
	}
	/* Unequal periods have no common boundary: cross-gating would defer one
	 * direction on every cycle and turn the gate into a source of late
	 * writes. Signal each direction at its own boundary instead. */
	if (!ff_gate_together(p->substream->runtime->period_size,
				c->substream->runtime->period_size)) {
		if (ff_pending(p))
			ff_signal_period(chip, p);
		if (ff_pending(c))
			ff_signal_period(chip, c);
		spin_unlock_irqrestore(&chip->gate_lock, flags);
		return;
	}
	{
		u32 mp = max((u32)(p->period - p->signalled),
			     (u32)(c->period - c->signalled));

		if (mp > chip->max_pending)
			chip->max_pending = mp;
	}
	if (ff_pending(p) && ff_pending(c)) {
		ff_signal_period(chip, p);
		ff_signal_period(chip, c);
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
	if (ff_pending(p) && ++p->deferrals >= 2) {
		atomic_inc(&chip->signal_overdue);
		ff_signal_period(chip, p);
		p->deferrals = 0;
	}
	if (ff_pending(c) && ++c->deferrals >= 2) {
		atomic_inc(&chip->signal_overdue);
		ff_signal_period(chip, c);
		c->deferrals = 0;
	}
	spin_unlock_irqrestore(&chip->gate_lock, flags);
}

/* Interrupt transport: one URB of interleaved frames. */
static void ff_capture_urb(struct ff_ep *ep, struct urb *urb)
{
	struct ff *chip = ep->chip;
	struct ff_stream *s = &chip->streams[SNDRV_PCM_STREAM_CAPTURE];
	struct snd_pcm_substream *subs = s->substream;
	const unsigned int words = chip->mode->words;
	const unsigned int total = chip->mode->neps * words;
	unsigned int frames = urb->actual_length / (words * FF_SAMPLE_BYTES);
	unsigned long flags;
	bool elapsed = false;
	bool bad = false;

	/* A remainder shorter than one frame carries no complete channel
	 * set; copying it would shift the channel order for the rest of
	 * the stream.  An overlong payload cannot exceed the buffer.  The
	 * floor is copied and the remainder discarded, so the ring and
	 * the wire stay frame-aligned; the anomaly is counted, like the
	 * playback side. */
	if (urb->actual_length % (words * FF_SAMPLE_BYTES))
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
			ff_ring_event(chip, subs, &chip->in_overruns,
					"capture overwrite", appl, own);
		if (chip->s32_samples)
			ff_capture_copy_int32(subs->runtime, s->pos[ep->index],
						urb->transfer_buffer, frames, words,
						ep->index * words, words, 1, total);
		else
			ff_capture_copy(subs->runtime, s->pos[ep->index],
					  urb->transfer_buffer, frames, words,
					  ep->index * words, words, 1, total);
		elapsed = ff_stream_advance(chip, s, subs->runtime,
					      ep->index, frames);
	}
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	if (elapsed)
		ff_period_elapsed(chip, SNDRV_PCM_STREAM_CAPTURE);
}

/* Startup prefill gate, evaluated under the stream lock at each playback
 * fill.  Returns true when the gate just cleared: the caller queues
 * prefill_work after unlocking (the mute op is a sleeping USB transfer).
 * The gate holds for the minimum window, then until enough consecutive
 * non-starved anchored URBs; a starved or unanchored URB resets the stable
 * count.  The bound guarantees the mute cannot stick if the host never
 * delivers.  With the parameter off, an armed gate clears at the next
 * fill. */
static bool ff_prefill_gate(struct ff *chip, struct ff_stream *s,
			      bool starved, bool anchored)
{
	unsigned int age;

	if (!s->out_prefill_pending)
		return false;
	if (!prefill) {
		s->out_prefill_pending = false;
		chip->ring_max_stall_ns = 0;
		return true;
	}
	if (!starved && anchored)
		s->out_prefill_stable++;
	else
		s->out_prefill_stable = 0;
	s->out_prefill_bound--;
	age = s->out_prefill_max - s->out_prefill_bound;
	if (age >= s->out_prefill_min &&
	    s->out_prefill_stable >= s->out_prefill_need) {
		s->out_prefill_pending = false;
		chip->ring_max_stall_ns = 0;
		atomic_inc(&chip->out_prefills);
		return true;
	}
	if (s->out_prefill_bound == 0) {
		s->out_prefill_pending = false;
		chip->ring_max_stall_ns = 0;
		atomic_inc(&chip->out_prefill_timeouts);
		return true;
	}
	return false;
}

static void ff_playback_urb(struct ff_ep *ep, struct urb *urb)
{
	struct ff *chip = ep->chip;
	struct ff_stream *s = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	struct snd_pcm_substream *subs = s->substream;
	const unsigned int words = ff_mode_out_words(chip->mode);
	unsigned int frames = urb->actual_length / (words * FF_SAMPLE_BYTES);
	unsigned long flags;
	bool elapsed = false;
	bool bad = false;
	bool starved = false;
	bool stop = false;
	bool unmute = false;

	/* The engine submitted this OUT URB with urb_frames frames, so the
	 * nominal case equals it.  Copy exactly what left the wire, and
	 * never more than one URB holds: a remainder shorter than one
	 * frame carries no complete channel set and must not shift the
	 * ring; an overlong payload cannot exceed the buffer.  Both
	 * anomalies are counted, like the capture side. */
	if (urb->actual_length % (words * FF_SAMPLE_BYTES))
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
			/* The muted startup transient is intended: starvation
			 * while the gate is armed sends silence without counting
			 * an underrun or arming the XRUN stop. */
			if (!s->out_prefill_pending) {
				if (!s->out_starve_streak)
					ff_ring_event(chip, subs,
							&chip->out_underruns,
							"playback underrun",
							appl, own);
				s->out_starve_streak++;
				stop = s->out_starve_streak >= 2 &&
				       !s->out_start_urbs;
			}
		} else {
			s->out_starve_streak = 0;
		}
		bool measure_stall = !s->out_prefill_pending;

		if (ff_prefill_gate(chip, s, starved, true))
			unmute = true;
		if (s->out_start_urbs)
			s->out_start_urbs--;
		ff_ring_trace(chip, subs, s->pos[ep->index], appl, own,
				frames, measure_stall);
		if (starved) {
			memset(urb->transfer_buffer, 0,
			       urb->transfer_buffer_length);
		} else if (chip->s32_samples) {
			ff_playback_copy_int32(subs->runtime, s->pos[ep->index],
						 urb->transfer_buffer, frames,
						 words, ep->index * words,
						 words);
			elapsed = ff_stream_advance(chip, s, subs->runtime,
						      ep->index, frames);
		} else {
			ff_playback_copy(subs->runtime, s->pos[ep->index],
					   urb->transfer_buffer, frames, words,
					   ep->index * words, words);
			elapsed = ff_stream_advance(chip, s, subs->runtime,
						      ep->index, frames);
		}
	} else {
		memset(urb->transfer_buffer, 0, urb->transfer_buffer_length);
	}
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	if (unmute)
		queue_work(system_unbound_wq, &chip->prefill_work);
	if (stop)
		snd_pcm_stop_xrun(subs);
	if (elapsed)
		ff_period_elapsed(chip, SNDRV_PCM_STREAM_PLAYBACK);
}

/* ISO implicit feedback: remember every IN packet's position and frame
 * count by packet number. */
static void ff_fb_put(struct ff *chip, unsigned int pos, unsigned int frames,
			unsigned int phase, bool seed)
{
	u32 e;

	/* Header layout per rate band: frame count bits 0-4, position bits
	 * 5..(4+width), phase the remaining upper bits (see ff_iso_layouts). */
	e = ((phase & ff_iso_phase_mask(chip)) << ff_iso_phase_shift(chip)) |
	    ((pos & ff_iso_pos_mask(chip)) << 5) | (frames & 0x1f);
	chip->fb[chip->in_pkts++ & (FF_FB_SIZE - 1)] = e;
	chip->in_end = (pos + frames) % ff_iso_wrap(chip);
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
			s32 jump = (s32)((pos + ff_iso_wrap(chip) -
					  (u32)(chip->in_field % ff_iso_wrap(chip))) %
					 ff_iso_wrap(chip));
			if (jump > (s32)(ff_iso_wrap(chip) / 2))
				jump -= ff_iso_wrap(chip);
			chip->in_field += jump + frames;
		}
	}
}

static void ff_fb_put_nominal(struct ff *chip)
{
	chip->fb_acc += ff_programmed_clock(chip);
	ff_fb_put(chip, chip->in_end, chip->fb_acc / 8000, 0, false);
	chip->fb_acc %= 8000;
}

static bool ff_fb_get(struct ff *chip, u64 k, unsigned int *pos,
			unsigned int *frames, unsigned int *phase)
{
	u64 src = k - chip->fb_delay;
	u32 e;

	if (k < chip->fb_delay || src >= chip->in_pkts ||
	    chip->in_pkts - src > FF_FB_SIZE)
		return false;
	e = chip->fb[src & (FF_FB_SIZE - 1)];
	*pos = (e >> 5) & ff_iso_pos_mask(chip);
	*phase = (e >> ff_iso_phase_shift(chip)) & ff_iso_phase_mask(chip);
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
static u32 ff_lead_seed(struct ff *chip)
{
	u64 wrap = ff_iso_wrap(chip);
	u64 step_q32 = div64_u64((u64)ff_programmed_clock(chip) << 32, 8000);
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
 * Whole-run mean frames per packet times 8000: the device clock the
 * packets reveal, kept for display.  The mismatch verdict (periodic guard
 * and stop verdict alike) uses the converged mean (ff-cadence.h), so a
 * rate-settling transient is not counted as a fault.
 */
/* The cadence judge's reference: the programmed clock (nominal rate scaled
 * by the pitch written through the DDS divisor). Falls back to the nominal
 * rate before any DDS word has been tracked. */
static unsigned int ff_programmed_clock(const struct ff *chip)
{
	unsigned int c = READ_ONCE(chip->clock_rate);

	return c ? c : chip->rate;
}

static unsigned int ff_iso_cadence(struct ff *chip, int *mismatch)
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
	if (mismatch && ff_cadence_mismatch(ff_programmed_clock(chip), mean))
		*mismatch = 1;
	return mean;
}

static void ff_iso_capture_urb(struct ff_ep *ep, struct urb *urb)
{
	struct ff *chip = ep->chip;
	struct ff_stream *s = &chip->streams[SNDRV_PCM_STREAM_CAPTURE];
	struct snd_pcm_substream *subs = s->substream;
	const struct ff_mode *m = chip->mode;
	const unsigned int nch = m->words;
	unsigned int frames[FF_MAX_ISO_PACKETS];
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
			ff_fb_put_nominal(chip);
			continue;
		}
		if (d->actual_length < FF_ISO_IN_HEADER) {
			atomic_inc(&chip->in_short);
			frames[i] = 0;
			ff_fb_put_nominal(chip);
			continue;
		}
		if ((d->actual_length - FF_ISO_IN_HEADER) %
		    (nch * FF_SAMPLE_BYTES)) {
			atomic_inc(&chip->in_misaligned);
			frames[i] = 0;
			ff_fb_put_nominal(chip);
			continue;
		}
		frames[i] = min((d->actual_length - FF_ISO_IN_HEADER) /
				(nch * FF_SAMPLE_BYTES), m->max_frames);
		if (!frames[i])
			atomic_inc(&chip->in_hdr_only);
		if (frames[i] < ARRAY_SIZE(chip->iso_frames_hist)) {
			chip->iso_frames_hist[frames[i]]++;
		} else {
			atomic_inc(&chip->in_frames_overflow);
		}
		/* Periodic cadence guard: the packets reveal the device
		 * clock; a mismatch means the rate write did not take
		 * effect or the device was reprogrammed externally. The
		 * verdict uses the converged (stationary) mean, like the
		 * stop verdict: a rate-settling transient is a moving mean
		 * that has not converged, so it is not counted. The counter
		 * flags only a steady cadence that disagrees with the
		 * committed rate, never a normal rate change. */
		if ((chip->in_pkts % 8000) == 0) {
			if (atomic_read(&chip->in_frames_overflow))
				goto cadence_skip;
			chip->iso_cadence = ff_iso_cadence(chip, NULL);
			if (ff_cadence_stop_mismatch(&chip->cad,
						       ff_programmed_clock(chip))) {
				if (++chip->rate_mismatch_windows >= 2) {
					atomic_inc(&chip->rate_mismatch);
					dev_warn_ratelimited(&chip->dev->dev,
							     "ISO cadence %u Hz vs clock %u Hz\n",
							     chip->cad.stable_mean,
							     ff_programmed_clock(chip));
					chip->rate_mismatch_windows = 0;
				}
			} else {
				chip->rate_mismatch_windows = 0;
			}
cadence_skip:
			;
		}
		hdr = le32_to_cpup((const __le32 *)pkt);
		/* Header layout per rate band (ff_iso_layouts): frame count
		 * bits 0-4, position bits 5..(4+width) wrapping at the band
		 * modulus, phase the remaining upper bits.  The wrap is a power
		 * of two, so the field mask and the modulo reduction agree.  A
		 * header-only packet carries the current position with zero
		 * frames; it must advance the feedback position, not reset it. */
		{
			unsigned int pos = (hdr >> 5) & ff_iso_pos_mask(chip);
			unsigned int hframes = hdr & 0x1f;
			unsigned int phase = (hdr >> ff_iso_phase_shift(chip)) &
					     ff_iso_phase_mask(chip);
			unsigned int wrap = ff_iso_wrap(chip);

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
			ff_fb_put(chip, pos, frames[i], phase, true);
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
		ff_cadence_window(&chip->cad, ff_programmed_clock(chip),
				    urb_frames, urb->number_of_packets);
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
			ff_ring_event(chip, subs, &chip->in_overruns,
					"capture overwrite", appl, own);
		for (i = 0; i < urb->number_of_packets; i++) {
			const u8 *pkt = (const u8 *)urb->transfer_buffer +
					urb->iso_frame_desc[i].offset;

			if (!frames[i])
				continue;
			if (chip->s32_samples)
				ff_capture_copy_int32(subs->runtime, s->pos[0],
					  (const __le32 *)(pkt + FF_ISO_IN_HEADER),
					  frames[i], nch, 0, 1, frames[i], nch);
			else
				ff_capture_copy(subs->runtime, s->pos[0],
					  (const __le32 *)(pkt + FF_ISO_IN_HEADER),
					  frames[i], nch, 0, 1, frames[i], nch);
			elapsed |= ff_stream_advance(chip, s, subs->runtime, 0,
						       frames[i]);
		}
	}
	snd_pcm_stream_unlock_irqrestore(subs, flags);
	if (elapsed)
		ff_period_elapsed(chip, SNDRV_PCM_STREAM_CAPTURE);
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
static void ff_iso_playback_fill(struct ff_ep *ep, struct urb *urb)
{
	struct ff *chip = ep->chip;
	struct ff_stream *s = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	struct snd_pcm_substream *subs = s->substream;
	const struct ff_mode *m = chip->mode;
	const unsigned int nch = ff_mode_out_words(m);
	const unsigned int fwords = nch + 1;
	unsigned int frames[FF_MAX_ISO_PACKETS], pos[FF_MAX_ISO_PACKETS];
	unsigned int phase[FF_MAX_ISO_PACKETS];
	unsigned int off = 0;
	unsigned long flags;
	bool elapsed = false;
	bool starved = false;
	bool stop = false;
	bool unmute = false;
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
		if (!ff_fb_get(chip, k, &pos[i], &frames[i], &phase[i])) {
			if (k < chip->fb_delay) {
				atomic_inc(&chip->out_fb_priming);
				/* Priming carries programmed-cadence frames
				 * (nominal scaled by the pitch), never empty
				 * packets: the device output buffer must stay
				 * fed before feedback exists. The fractional
				 * accumulator advances at the exact programmed
				 * clock, so a re-prepare under pitch primes at
				 * the rate the device consumes and the handover
				 * to the mirrored counts carries no cadence
				 * step. Priming at nominal under pitch leaves a
				 * deficit that starves the ring and churns XRUN
				 * recovery (each recovery primes again). */
				chip->out_prime_acc += ff_programmed_clock(chip);
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
			unsigned int len = frames[i] * fwords * FF_SAMPLE_BYTES;

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
			/* The muted startup transient is intended: starvation
			 * while the gate is armed sends silence without counting
			 * an underrun or arming the XRUN stop. */
			if (!s->out_prefill_pending) {
				if (!s->out_starve_streak)
					ff_ring_event(chip, subs,
							&chip->out_underruns,
							"playback underrun",
							appl, own);
				s->out_starve_streak++;
				stop = s->out_starve_streak >= 2 &&
				       !s->out_start_urbs;
			}
		} else {
			s->out_starve_streak = 0;
		}
		bool measure_stall = !s->out_prefill_pending;

		if (ff_prefill_gate(chip, s, starved, s->out_counter_valid))
			unmute = true;
		if (s->out_start_urbs)
			s->out_start_urbs--;
		ff_ring_trace(chip, subs, s->hw_ptr, appl, own, total,
				measure_stall);
	}
	/* The seed needs a real device timestamp: ts_pos_valid means at
	 * least one capture header arrived.  Priming frames are nominal, so
	 * a non-zero frames[] no longer implies feedback; without the
	 * timestamp the rewind would extrapolate from a zero position. */
	if (s->running && !s->out_counter_valid && chip->ts_pos_valid) {
		for (i = 0; i < urb->number_of_packets; i++) {
			if (frames[i]) {
				s->out_counter = ff_lead_seed(chip);
				s->out_counter_valid = true;
				break;
			}
		}
	}
	/* The in-flight queue for .pointer's delay (R4): this URB's
	 * playback frames, added at submit and removed at completion. */
	{
		struct ff_urb *u = urb->context;
		unsigned int total = 0;

		for (i = 0; i < urb->number_of_packets; i++)
			total += frames[i];
		if (u)
			u->frames = total;
	}
	for (i = 0; i < urb->number_of_packets; i++) {
		__le32 *dst = (__le32 *)((u8 *)urb->transfer_buffer + off);
		unsigned int len = frames[i] * fwords * FF_SAMPLE_BYTES;
		unsigned int f;

		urb->iso_frame_desc[i].offset = off;
		urb->iso_frame_desc[i].length = len;
		if (s->running && frames[i] && !starved &&
		    s->out_counter_valid) {
			if (chip->s32_samples)
				ff_playback_copy_int32(subs->runtime, s->pos[0],
					   dst + 1, frames[i], nch, 0, fwords);
			else
				ff_playback_copy(subs->runtime, s->pos[0],
					   dst + 1, frames[i], nch, 0, fwords);
			elapsed |= ff_stream_advance(chip, s, subs->runtime, 0,
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

			if (chip->caps & FF_TCAP_FRAME_LEAD) {
				for (f = 0; f < frames[i]; f++)
					dst[f * fwords] = cpu_to_le32(
						(base + f) %
						ff_iso_wrap(chip));
			} else {
				dst[0] = cpu_to_le32(base);
			}
			s->out_counter += frames[i];
			/* The lag watermarks describe the anchored counter
			 * only: priming packets before the seed carry lead
			 * words that are not placed against the device
			 * timeline. */
			if (s->out_counter_valid) {
				unsigned int wrap = ff_iso_wrap(chip);
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
	if (unmute)
		queue_work(system_unbound_wq, &chip->prefill_work);
	/* Report the XRUN to ALSA so the application recovers through the
	 * normal prepare/start path.  The engine keeps running and feeds the
	 * device silence; only the host PCM stream stops.  Called outside the
	 * stream lock: snd_pcm_stop_xrun takes it itself. */
	if (stop)
		snd_pcm_stop_xrun(subs);
	if (elapsed)
		ff_period_elapsed(chip, SNDRV_PCM_STREAM_PLAYBACK);
}

/*
 * ISO URB continuity.  start_frame and number_of_packets are in the
 * units the HCD reports; the watchdog only compares successive URBs, so
 * any consistent unit cancels.  The first log line of a run records both
 * values so the unit can be confirmed from the bench.
 */
static bool ff_iso_contiguous(struct ff_ep *ep, struct urb *urb)
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
			 FF_ISO_FRAME_MASK;
	return expected < 0 ||
	       ((urb->start_frame - expected) & FF_ISO_FRAME_MASK) == 0;
}

static void ff_stop_running_streams(struct ff *chip)
{
	int dir;

	for (dir = 0; dir < 2; dir++) {
		struct snd_pcm_substream *subs = chip->streams[dir].substream;

		if (subs)
			snd_pcm_stop_xrun(subs);
	}
}

static void ff_track_playback_urb(struct ff *chip, struct urb *urb)
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

static void ff_track_capture_urb(struct ff *chip, struct urb *urb)
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

static int ff_stats_show(struct seq_file *m, void *v)
{
	struct ff *chip = m->private;

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
	seq_printf(m, "pitch_clamps %d\n", atomic_read(&chip->pitch_clamps));
	seq_printf(m, "out_prefills %d\nout_prefill_timeouts %d\n",
		   atomic_read(&chip->out_prefills),
		   atomic_read(&chip->out_prefill_timeouts));
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
DEFINE_SHOW_ATTRIBUTE(ff_stats);

static int ff_clock_show(struct seq_file *m, void *v)
{
	struct ff *chip = m->private;
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
		   (unsigned long long)frac);
	seq_printf(m, "rate_meas %u\n", rate);
	seq_printf(m, "clock_rate %u\n", ff_programmed_clock(chip));
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
	seq_printf(m, "fb_delay %u out_lead %u lead_margin %d\n",
		   chip->fb_delay, chip->out_lead, lead_margin);
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(ff_clock);

static int ff_ringtrace_show(struct seq_file *m, void *v)
{
	struct ff *chip = m->private;
	struct snd_pcm_substream *subs =
		READ_ONCE(chip->streams[SNDRV_PCM_STREAM_PLAYBACK].substream);
	struct ff_ring_rec *snap;
	unsigned long flags;
	u32 head, n, i;

	snap = kmalloc_array(FF_RING_TRACE, sizeof(*snap), GFP_KERNEL);
	if (!snap)
		return -ENOMEM;
	if (subs)
		snd_pcm_stream_lock_irqsave(subs, flags);
	head = chip->ring_head;
	n = min(head, (u32)FF_RING_TRACE);
	for (i = 0; i < n; i++)
		snap[i] = chip->ring_trace[(head - n + i) % FF_RING_TRACE];
	if (subs)
		snd_pcm_stream_unlock_irqrestore(subs, flags);

	seq_printf(m, "# %u completions total, last %u shown\n", head, n);
	seq_printf(m, "# min_filled %u missed %d starved %d max_stall_ns %llu\n",
		   chip->ring_min_filled, atomic_read(&chip->ring_missed),
		   atomic_read(&chip->ring_starved),
		   (unsigned long long)chip->ring_max_stall_ns);
	seq_puts(m, "# ktime_ns appl hw status_hw filled total flags\n");
	for (i = 0; i < n; i++) {
		const struct ff_ring_rec *r = &snap[i];

		seq_printf(m, "%llu %llu %llu %u %u %u %u\n",
			   (unsigned long long)r->ktime_ns,
			   (unsigned long long)r->appl,
			   (unsigned long long)r->hw, r->status_hw,
			   r->filled, r->total, r->flags);
	}
	kfree(snap);
	return 0;
}
DEFINE_SHOW_ATTRIBUTE(ff_ringtrace);

static void ff_urb_complete(struct urb *urb)
{
	struct ff_urb *u = urb->context;
	struct ff_ep *ep = u->ep;
	struct ff *chip = ep->chip;
	int ret;

	if (urb->status != -ENOENT && urb->status != -ECONNRESET &&
	    urb->status != -ESHUTDOWN && urb->status != -ENODEV) {
		if (ep->dir == SNDRV_PCM_STREAM_PLAYBACK)
			ff_track_playback_urb(chip, urb);
		else
			ff_track_capture_urb(chip, urb);
	}

	if (u->frames && ep->dir == SNDRV_PCM_STREAM_PLAYBACK) {
		atomic_sub(u->frames, &ep->inflight);
		atomic_sub(u->frames, &chip->streams[
			SNDRV_PCM_STREAM_PLAYBACK].inflight);
	}

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
		if (atomic_inc_return(&chip->urb_errors) > FF_MAX_URB_ERRORS) {
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
			ff_iso_capture_urb(ep, urb);
		else
			ff_capture_urb(ep, urb);
	} else {
		if (chip->iso)
			ff_iso_playback_fill(ep, urb);
		else
			ff_playback_urb(ep, urb);
	}

resubmit:
	if (!READ_ONCE(chip->engine_running))
		return;
	ret = usb_submit_urb(urb, GFP_ATOMIC);
	if (ret == 0) {
		if (!ep->dir) {
			struct ff_urb *uu = urb->context;

			if (uu && uu->frames) {
				atomic_add(uu->frames, &ep->inflight);
				atomic_add(uu->frames,
					   &chip->streams[SNDRV_PCM_STREAM_PLAYBACK].inflight);
			}
		}
		if (!READ_ONCE(chip->iso_frames_ok) ||
		    ff_iso_contiguous(ep, urb))
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

static void ff_status_complete(struct urb *urb);

static int ff_status_submit(struct ff *chip)
{
	int ret;

	if (!chip->status_intf || chip->status_active ||
	    READ_ONCE(chip->disconnected))
		return 0;
	if (chip->desc->status_bulk)
		usb_fill_bulk_urb(chip->status_urb, chip->dev,
				  usb_rcvbulkpipe(chip->dev,
						  chip->desc->status_ep),
				  chip->status_buf, FF_STATUS_LEN,
				  ff_status_complete, chip);
	else
		usb_fill_int_urb(chip->status_urb, chip->dev,
				 usb_rcvintpipe(chip->dev,
						chip->desc->status_ep),
				 chip->status_buf, FF_STATUS_LEN,
				 ff_status_complete, chip, 1);
	WRITE_ONCE(chip->status_armed_at,
		   jiffies + msecs_to_jiffies(FF_STATUS_GRACE_MS));
	ret = usb_submit_urb(chip->status_urb, GFP_ATOMIC);
	if (!ret)
		chip->status_active = true;
	return ret;
}

static void ff_status_complete(struct urb *urb)
{
	struct ff *chip = urb->context;
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

	if (urb->actual_length >= FF_STATUS_LEN) {
		memcpy(chip->last_status, chip->status_buf, FF_STATUS_LEN);
		ff_event_put(chip, FF_EV_STATUS, chip->status_buf,
			       min_t(unsigned int, urb->actual_length,
				     FF_EVENT_MAX), 0);
	}

	latched = (chip->desc->quirks & FF_QUIRK_STATUS_LATCH) &&
		  chip->last_status[3] & FF_STATUS_OUT_LATCHED;
	if (latched != READ_ONCE(chip->out_latched)) {
		WRITE_ONCE(chip->out_latched, latched);
		ff_event_put(chip, FF_EV_LATCH, NULL, 0,
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
	if ((chip->caps & FF_TCAP_STATUS_RUNNING_ONLY) &&
	    !READ_ONCE(chip->engine_running))
		return;
	ret = ff_status_submit(chip);
	if (ret < 0 && ret != -EPERM)
		dev_dbg_ratelimited(&chip->dev->dev,
				    "status URB resubmit failed: %d\n", ret);
}

static void ff_kill_urbs(struct ff *chip, bool kill_status)
{
	int dir, e, i;

	for (dir = 0; dir < 2; dir++)
		for (e = 0; e < FF_MAX_EPS; e++)
			for (i = 0; i < chip->nurbs; i++)
				usb_kill_urb(chip->eps[dir][e].urbs[i].urb);
	if (kill_status) {
		usb_kill_urb(chip->status_urb);
		chip->status_active = false;
	}
}

static void ff_engine_stop(struct ff *chip)
{
	int i, j;

	if (!chip->engine_running)
		return;
	/* A restarted engine has a new frame counter; the ISO continuity
	 * watchdog must not compare across the restart. */
	for (i = 0; i < 2; i++)
		for (j = 0; j < FF_MAX_EPS; j++)
			chip->eps[i][j].next_start = -1;
	for (i = 0; i < 2; i++)
		for (j = 0; j < FF_MAX_EPS; j++)
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
		chip->desc->ops->stream_stop(chip, ff_mode_channels(chip,
								     chip->rate));
		if (chip->caps & FF_TCAP_MUTE_ON_STOP)
			chip->desc->ops->hw_mute(chip, true);
	}
	WRITE_ONCE(chip->engine_running, false);
	ff_kill_urbs(chip, chip->caps & FF_TCAP_KILL_BOTH);
	/* The kill is synchronous: every in-flight frame has been
	 * subtracted by its completion.  Rebase the counter for the next
	 * engine run. */
	atomic_set(&chip->streams[SNDRV_PCM_STREAM_PLAYBACK].inflight, 0);
	atomic_set(&chip->streams[SNDRV_PCM_STREAM_CAPTURE].inflight, 0);
	{
		int d, k;

		for (d = 0; d < 2; d++)
			for (k = 0; k < FF_MAX_EPS; k++)
				atomic_set(&chip->eps[d][k].inflight, 0);
	}
	chip->engine_stopped_at = jiffies ?: 1;
	dev_dbg(&chip->dev->dev, "engine stopped\n");
}

static void ff_int_setup_urbs(struct ff *chip, unsigned int bytes)
{
	const struct ff_mode *m = chip->mode;
	unsigned int in_bytes = bytes;
	int dir, e, i;

	/* The input word count follows the current alternate setting: 18 at
	 * 1x, 14 at 2x and 6 per input endpoint at 4x.  The receive URB is
	 * the same scaled frame count as the playback URB; using the maximum
	 * across all modes mis-sizes the 2x/4x receive URB. */
	if (chip->caps & FF_TCAP_FIXED_URB)
		in_bytes = chip->urb_frames * m->words * FF_SAMPLE_BYTES;

	for (dir = 0; dir < 2; dir++) {
		for (e = 0; e < m->neps; e++) {
			struct ff_ep *ep = &chip->eps[dir][e];
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
				struct ff_urb *u = &ep->urbs[i];

				usb_fill_int_urb(u->urb, chip->dev, pipe, u->buf,
						 len, ff_urb_complete, u, 1);
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

static void ff_iso_setup_urbs(struct ff *chip)
{
	const struct ff_mode *m = chip->mode;
	int dir, i, p;

	chip->in_pkts = chip->out_pkts = 0;
	chip->streams[SNDRV_PCM_STREAM_PLAYBACK].out_counter_valid = false;
	chip->iso_frames_ok = false;
	chip->in_end = 0;
	chip->in_start_resync = false;
	chip->fb_acc = 0;
	chip->out_prime_acc = 0;
	chip->fb_delay = ff_iso_fb_delay(chip);
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
	/* Per-run diagnostics: a stop line describes this stream only.  The
	 * rate commit above already sent the DDS word, and ff_track_clock
	 * derived the programmed clock from it; do not overwrite a pitched
	 * reference with the nominal rate here. */
	memset(chip->iso_frames_hist, 0, sizeof(chip->iso_frames_hist));
	ff_cadence_reset(&chip->cad);
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
	 * mirrors.  The reference host's OUT ring lead bias is
	 * 2 * max_frames - 2 (12/24/48 at 1x/2x/4x), added to every lead word
	 * and set at engine start; it is the same for UC, UCX and
	 * Babyface-classic.  A bias below it underruns.  The lead_margin
	 * parameter overrides for A/B.  Reduce modulo the wrap (8192 at 1x). */
	chip->out_lead = ff_iso_out_lead(chip);

	for (dir = 0; dir < 2; dir++) {
		struct ff_ep *ep = &chip->eps[dir][0];

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
			urb->complete = ff_urb_complete;
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
				ff_iso_playback_fill(ep, urb);
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

static void ff_iso_align(struct ff *chip)
{
	struct ff_ep *ein = &chip->eps[SNDRV_PCM_STREAM_CAPTURE][0];
	struct ff_ep *eout = &chip->eps[SNDRV_PCM_STREAM_PLAYBACK][0];
	int in = ein->urbs[0].urb->start_frame;
	int out = eout->urbs[0].urb->start_frame;
	int diff = out - in;
	bool ok = true;
	unsigned long flags;
	int i;

	for (i = 0; i < chip->nurbs; i++) {
		ok &= ff_iso_contiguous(ein, ein->urbs[i].urb);
		ok &= ff_iso_contiguous(eout, eout->urbs[i].urb);
	}
	if (!ok || abs(diff) > 64)
		diff = 0;
	spin_lock_irqsave(&chip->fb_lock, flags);
	chip->out_lead = (chip->out_lead + ff_iso_wrap(chip) +
			  diff * (int)chip->rate / 8000) %
			 ff_iso_wrap(chip);
	spin_unlock_irqrestore(&chip->fb_lock, flags);
	WRITE_ONCE(chip->iso_frames_ok, ok);
	dev_dbg(&chip->dev->dev, "start frames IN %d OUT %d%s, OUT lead %u\n",
		in, out, ok ? "" : " (not contiguous, unchecked)", chip->out_lead);
}

/* Commit the adopted rate to the device with nothing running: the
 * Windows-personality restart settle, the alternate-setting (re)selection
 * and the rate sequence.  Shared by engine start and the idle device-rate
 * write so both establish the same device clock state (one transition,
 * every path).  The caller holds chip->mutex and has set chip->rate and
 * chip->mode to the target. */
static int ff_engine_commit_rate(struct ff *chip)
{
	const struct ff_mode *m = chip->mode;
	int ret;

	/* PipeWire reconfigures both nodes for a new quantum by closing and
	 * reopening them within milliseconds.  Windows leaves seconds
	 * between a session close and the next open; a start that follows
	 * the close word at once (interface reselect, session words) leaves
	 * a UC_WIN_RME unit dead in both directions until the module is
	 * reloaded.  100 ms was enough on the bench; keep a margin.  The
	 * settle is a Windows-personality property (FF_QUIRK_WIN_SESSION);
	 * the AP personality restarts without it. */
	if ((chip->desc->quirks & FF_QUIRK_WIN_SESSION) &&
	    chip->engine_stopped_at) {
		unsigned long ready = chip->engine_stopped_at +
				      msecs_to_jiffies(FF_RESTART_SETTLE_MS);

		if (time_before(jiffies, ready))
			msleep(jiffies_to_msecs(ready - jiffies) + 1);
	}

	/* alt 0 -> alt N reselect clears the UCX IF5 output latch. */
	if (chip->desc->quirks & FF_QUIRK_ALT_RESELECT) {
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
		bool skip_alt = (chip->caps & FF_TCAP_SKIP_SAME_ALT) &&
				chip->uc_session &&
				chip->cur_alt == m->alt;

		dev_dbg(&chip->dev->dev,
			"rate commit: rate=%u alt=%u cur_alt=%u session=%d skip_alt=%d\n",
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
	/* Commit the rate on every start.  The device keeps its own rate
	 * across personality switches and prior sessions, and the init
	 * sequence does not set it, so a cached value cannot be trusted.
	 * The state word and 0x19 word report the selected family/band, not
	 * the actual DDS clock; only the DDS word moves the clock.  The
	 * sequence therefore writes the DDS and the family selector on every
	 * start.  The ISO form leaves the options word to the settings path;
	 * the interrupt form keeps the Windows options commit.  This point is
	 * reached only with nothing running, so the commit
	 * cannot glitch a live stream.  On a Windows-session unit the close
	 * word needs the Windows gap before the rate write (A2: 220 ms);
	 * writing right after the close wedges the unit.  The settle above
	 * already separated the close from this start. */
	if ((chip->desc->quirks & FF_QUIRK_WIN_SESSION) && chip->uc_session)
		msleep(200);
	atomic_inc(&chip->rate_commits);
	ret = chip->desc->ops->set_rate(chip, chip->rate);
	dev_dbg(&chip->dev->dev,
		"rate commit: %u Hz alt %u cur_alt %u ret=%d\n",
		chip->rate, m->alt, chip->cur_alt, ret);
	return ret;
}

static int ff_engine_start(struct ff *chip)
{
	const struct ff_mode *m = chip->mode;
	unsigned int bytes;
	int e, i, ret;

	if (chip->engine_running)
		return 0;
	/* Fresh continuity expectation for a fresh engine. */
	for (e = 0; e < 2; e++)
		for (i = 0; i < FF_MAX_EPS; i++) {
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

	chip->urb_frames = ff_urb_frames(chip, chip->rate);
	if (chip->iso)
		bytes = chip->iso_packets * max(m->in_mps, m->out_mps);
	else
		bytes = chip->urb_frames *
			max(m->words, ff_mode_out_words(m)) *
			FF_SAMPLE_BYTES;
	if (WARN_ON(bytes > chip->urb_bytes_max))
		return -EINVAL;
	ret = ff_engine_commit_rate(chip);
	if (ret < 0)
		return ret;

	/* Windows sends the session-open trio before the stream starts, not
	 * after the URBs are submitted.  On the UC the trio is what opens the
	 * device's audio path at all; sending it after the endpoints are
	 * already running leaves the stream alive with silence in both
	 * directions.  The later stream_start call is a no-op for the UC once
	 * session_reopen has been cleared. */
	if ((chip->caps & FF_TCAP_SESSION_BEFORE_URBS) && chip->uc_session) {
		/* Windows leaves seconds between a stream close and the next
		 * open (A2b: 6.64 -> 10.87).  A back-to-back reopen after the
		 * 0x13 close leaves the UC silent even when the rate did not
		 * change.  Give the close time to take effect.  The gap is a
		 * Windows-personality property; the AP personality keeps the
		 * session words without it. */
		if (chip->desc->quirks & FF_QUIRK_WIN_SESSION)
			msleep(200);
		ret = chip->desc->ops->stream_start(
			chip, ff_mode_channels(chip, chip->rate));
		if (ret < 0)
			return ret;
	}

	if (chip->iso)
		ff_iso_setup_urbs(chip);
	else
		ff_int_setup_urbs(chip, bytes);

	/* The device resets its capture stream counter after the host stops
	 * polling.  Let the reset complete before the first URB so the run
	 * starts on a clean zero-based timeline, not the reset's position-0
	 * placeholders and error microframe (DRIVER-DESIGN §4.3, §7). */
	if (chip->iso)
		msleep(FF_ISO_START_MARGIN_MS);

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
				struct ff_urb *u =
					chip->eps[SNDRV_PCM_STREAM_PLAYBACK][e]
						.urbs[i].urb->context;

				if (u->frames) {
					atomic_add(u->frames,
						   &chip->eps[SNDRV_PCM_STREAM_PLAYBACK][e].inflight);
					atomic_add(u->frames,
						   &chip->streams[SNDRV_PCM_STREAM_PLAYBACK].inflight);
				}
			}
		}
	}

	/* The UC already sent its session-open trio before the URBs.  The
	 * UCX still needs its stream_start after the URBs are submitted. */
	if (!(chip->caps & FF_TCAP_SESSION_BEFORE_URBS)) {
		ret = chip->desc->ops->stream_start(
			chip, ff_mode_channels(chip, chip->rate));
		if (ret < 0)
			goto error;
	}

	if (chip->iso)
		ff_iso_align(chip);

	WRITE_ONCE(chip->status_armed_at,
		   jiffies + msecs_to_jiffies(FF_STATUS_GRACE_MS));
	ret = ff_status_submit(chip);
	if (ret < 0)
		dev_warn_ratelimited(&chip->dev->dev,
				     "cannot watch the status endpoint: %d\n", ret);

	/* The startup gate owns the playback mute. With it active, mute here
	 * and let the gate unmute once delivery is stable: unmuting now would
	 * open an audible window between engine start and the gate's mute at
	 * trigger (the stream-start DAC-open sequence unmutes too). Without the
	 * gate, unmute now so the engine plays. */
	if (chip->desc->ops->hw_mute) {
		ret = chip->desc->ops->hw_mute(chip, prefill);
		if (ret < 0)
			goto error;
	}

	dev_dbg(&chip->dev->dev,
		"engine started: %u Hz, alt %u, %u frames x %u URBs\n",
		chip->rate, m->alt, chip->urb_frames, chip->nurbs);
	return 0;

error:
	dev_err(&chip->dev->dev, "failed to start streaming: %d\n", ret);
	WRITE_ONCE(chip->engine_running, false);
	ff_kill_urbs(chip, chip->caps & FF_TCAP_KILL_BOTH);
	return ret;
}

/* Startup prefill gate: apply the playback mute state.  The mute op is a
 * sleeping USB transfer; trigger and fill run in atomic context and only
 * queue this work.  State-driven and idempotent: the latest gate state
 * wins, and a stopped engine is left alone (engine_stop owns the mute). */
static void ff_prefill_work(struct work_struct *work)
{
	struct ff *chip = container_of(work, struct ff, prefill_work);

	mutex_lock(&chip->mutex);
	if (READ_ONCE(chip->engine_running) && chip->desc->ops->hw_mute)
		chip->desc->ops->hw_mute(chip,
			chip->streams[SNDRV_PCM_STREAM_PLAYBACK].out_prefill_pending);
	mutex_unlock(&chip->mutex);
}

static void ff_recover_work(struct work_struct *work)
{
	struct ff *chip = container_of(work, struct ff, recover_work);
	int ret;

	mutex_lock(&chip->mutex);
	if (READ_ONCE(chip->disconnected) || !chip->engine_running ||
	    !READ_ONCE(chip->engine_broken))
		goto unlock;

	chip->recoveries++;
	dev_warn_ratelimited(&chip->dev->dev,
			     "stream interrupted (output underrun or USB error), restarting (%u)\n",
			     chip->recoveries);
	ff_stop_running_streams(chip);
	ff_engine_stop(chip);
	if (chip->prepared_mask) {
		ret = ff_engine_start(chip);
		if (ret < 0)
			dev_err(&chip->dev->dev, "restart failed: %d\n", ret);
	}
unlock:
	mutex_unlock(&chip->mutex);
}

/* -- PCM ------------------------------------------------------------- */

static const struct snd_pcm_hardware ff_pcm_hw = {
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
	.rate_max = FF_MAX_RATE,
	.channels_min = 1,
	.channels_max = FF_MAX_CHANNELS,
	.buffer_bytes_max = 4 * 1024 * 1024,
	.period_bytes_min = 32,
	.period_bytes_max = 1024 * 1024,
	.periods_min = 2,
	.periods_max = 1024,
};

static int ff_rule_channels(struct snd_pcm_hw_params *params,
			      struct snd_pcm_hw_rule *rule)
{
	struct snd_pcm_substream *subs = rule->private;
	struct ff *chip = snd_pcm_substream_chip(subs);
	struct snd_interval *r = hw_param_interval(params, SNDRV_PCM_HW_PARAM_RATE);
	struct snd_interval t = {
		.min = 1,
		.max = ff_max_channels(chip, subs->stream, r->min),
		.integer = 1,
	};

	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_CHANNELS), &t);
}

static int ff_rule_rate(struct snd_pcm_hw_params *params,
			  struct snd_pcm_hw_rule *rule)
{
	struct snd_pcm_substream *subs = rule->private;
	struct ff *chip = snd_pcm_substream_chip(subs);
	unsigned int other = 1 << !subs->stream;
	struct snd_interval t = {
		.min = 32000,
		.max = min(chip->desc->max_rate, (unsigned int)FF_MAX_RATE),
		.integer = 1,
	};
	unsigned int locked = 0;

	if (READ_ONCE(chip->hw_params_mask) & other)
		locked = READ_ONCE(chip->rate);
	else if (ff_rate_supported(chip, READ_ONCE(fixed_rate)))
		locked = READ_ONCE(fixed_rate);
	if (locked) {
		t.min = max(t.min, locked);
		t.max = min(t.max, locked);
	}
	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_RATE), &t);
}

static int ff_rule_period(struct snd_pcm_hw_params *params,
			    struct snd_pcm_hw_rule *rule)
{
	struct ff *chip = rule->private;
	const struct snd_interval *r =
		hw_param_interval_c(params, SNDRV_PCM_HW_PARAM_RATE);
	struct snd_interval *p =
		hw_param_interval(params, SNDRV_PCM_HW_PARAM_PERIOD_SIZE);
	unsigned int step = ff_urb_frames(chip, r->min);
	struct snd_interval t = { .integer = 1 };

	/*
	 * The ALSA period and the USB URB are separate buffers.  On the
	 * interrupt transport a period is a whole number of URBs, so every
	 * period interrupt lands on a completion.  The ISO path keeps a
	 * one-URB floor and accepts any larger period.
	 */
	if (chip->caps & FF_TCAP_FIXED_URB) {
		if (p->max < step)
			return -EINVAL;
		t.min = roundup(p->min + p->openmin, step);
		t.max = rounddown(p->max - p->openmax, step);
		if (t.min > t.max)
			return -EINVAL;
		return snd_interval_refine(p, &t);
	}

	t.min = step;
	t.max = UINT_MAX;
	return snd_interval_refine(p, &t);
}

static unsigned int ff_buffer_min(struct ff *chip, unsigned int rate)
{
	/*
	 * The ALSA ring must cover the USB URBs already in flight plus two
	 * more URBs of margin.  On the UCX transport it must also cover the
	 * output lead horizon.  The capacity floor uses a conservative
	 * 60-frame lead margin, independent of the lead_bias default, plus
	 * two URBs beyond it, kept URB-aligned.  This is a minimum capacity,
	 * not the steady fill; the host normally requests a larger ring.
	 */
	unsigned int urb = ff_urb_frames(chip, rate);
	unsigned int min = (chip->nurbs + 2) * urb;

	/* The lead horizon is an ISO scheduling property.  The interrupt
	 * transport is paced by the device's fixed URB slices and keeps only
	 * the in-flight URBs plus margin. */
	if ((chip->caps & FF_TCAP_LEAD_HORIZON) &&
	    !(chip->caps & FF_TCAP_FIXED_URB)) {
		unsigned int lead = DIV_ROUND_CLOSEST(
			ff_iso_fb_delay(chip) * rate, 8000) + 60;

		min = max(min, lead + 2 * urb);
	}

	/* roundup() rounds to any multiple; round_up() is a power-of-two
	 * bitmask and would mis-align a non-power-of-two URB. */
	return roundup(min, urb);
}

static int ff_rule_buffer(struct snd_pcm_hw_params *params,
			    struct snd_pcm_hw_rule *rule)
{
	struct ff *chip = rule->private;
	struct snd_interval *r = hw_param_interval(params, SNDRV_PCM_HW_PARAM_RATE);
	struct snd_interval t = {
		.min = ff_buffer_min(chip, r->min),
		.max = UINT_MAX,
		.integer = 1,
	};

	return snd_interval_refine(hw_param_interval(params,
				   SNDRV_PCM_HW_PARAM_BUFFER_SIZE), &t);
}

/* The achievable rate/quantum envelope, reported through hwdep so userspace
 * presents exactly what the driver accepts.  Uses the same geometry functions
 * as the hw_params rules: one implementation, one owner. */
void ff_query_envelope(struct ff *chip, struct ff_hwdep_envelope *env)
{
	unsigned int i, n = 0;

	memset(env, 0, sizeof(*env));
	env->urb_frames = chip->base_urb_frames;
	env->nurbs = chip->nurbs;
	env->iso = chip->iso ? 1 : 0;
	env->iso_packets = chip->iso_packets;
	env->fb_delay = chip->iso ? ff_iso_fb_delay(chip) : chip->fb_delay;
	env->out_lead = chip->iso && !READ_ONCE(chip->engine_running) ?
		ff_iso_out_lead(chip) : chip->out_lead;
	env->lead_bias = chip->mode ? 2 * chip->mode->max_frames - 2 : 0;
	for (i = 0; i < ARRAY_SIZE(ff_rates) && n < FF_HWDEP_MAX_RATES; i++) {
		unsigned int rate = ff_rates[i];

		if (!ff_rate_supported(chip, rate))
			continue;
		const struct ff_mode *m = &chip->modes[ff_mode_index(rate)];

		env->rates[n].rate = rate;
		env->rates[n].period_floor = ff_urb_frames(chip, rate);
		env->rates[n].buffer_min = ff_buffer_min(chip, rate);
		env->rates[n].capture_channels = m->neps * m->words;
		env->rates[n].playback_channels =
			m->neps * ff_mode_out_words(m);
		n++;
	}
	env->count = n;
}

/* Confirm the committed rate from the state-word rate readback: poll the
 * internal-clock rate index until it matches, bounded.  The readiness
 * condition is the device's own rate report, not a timed guess. */
static int ff_confirm_idle_rate(struct ff *chip, unsigned int rate,
				  unsigned int *confirmed)
{
	unsigned long deadline = jiffies +
				 msecs_to_jiffies(FF_IDLE_RATE_CONFIRM_MS);
	unsigned int read_rate = 0;
	__le32 word;
	int ret;

	for (;;) {
		ret = ff_vendor_recv(chip, FF_REQ_STATUS, 0, 0, &word,
				       sizeof(word));
		if (ret == 0) {
			read_rate = ff_rate_from_index(le32_to_cpu(word) >> 24);
			if (read_rate == rate) {
				*confirmed = read_rate;
				return 0;
			}
		}
		if (time_after(jiffies, deadline))
			break;
		msleep(20);
	}
	*confirmed = read_rate;
	return -EIO;
}

/* The server-free idle device-rate write (the JACK / direct-ALSA pro path).
 * Precondition: the engine is idle, no owner holds the PCM.  Action: adopt
 * the rate and commit it to the device.  Postcondition: the state-word
 * readback confirms the rate; a run_seq bump arms the host scene reapply.
 * Idle-only: while an owner runs it sets the rate and this refuses (-EBUSY).
 * Not used under PipeWire, which forces the graph rate. */
int ff_set_rate_idle(struct ff *chip, unsigned int rate,
		       unsigned int *confirmed)
{
	unsigned int old_rate, old_clock;
	const struct ff_mode *old_mode;
	int ret;

	*confirmed = 0;
	if (!ff_rate_supported(chip, rate))
		return -EINVAL;

	mutex_lock(&chip->mutex);
	if (READ_ONCE(chip->disconnected)) {
		ret = -ENODEV;
		goto unlock;
	}
	/* Idle-only: an owner holding the PCM sets the rate itself. */
	if (chip->engine_running || chip->hw_params_mask || chip->prepared_mask) {
		ret = -EBUSY;
		goto unlock;
	}

	old_rate = chip->rate;
	old_clock = chip->clock_rate;
	old_mode = chip->mode;
	chip->rate = rate;
	chip->clock_rate = rate;
	chip->mode = &chip->modes[ff_mode_index(rate)];
	/* The UC session words follow the rate-change path: the next engine
	 * start re-opens the session at the new rate.  The session-close word
	 * was already sent when the stream stopped (or there was no session).
	 * UC session behaviour on a host-initiated idle rate change is a
	 * needs-hardware item; this mirrors the verified hw_params flow. */
	if (chip->caps & FF_TCAP_SESSION_ON_RATE_CHANGE) {
		chip->session_reopen = true;
		chip->uc_session = true;
	}

	mutex_lock(&chip->ctl_lock);
	ret = ff_engine_commit_rate(chip);
	if (ret == 0) {
		int cret = ff_confirm_idle_rate(chip, rate, confirmed);

		if (cret < 0)
			/* Committed but unconfirmed: the device moved, so keep
			 * the adopt and report the readback honestly. */
			ret = cret;
	} else {
		/* The device write failed: restore the adopted state. */
		chip->rate = old_rate;
		chip->clock_rate = old_clock;
		chip->mode = old_mode;
	}
	mutex_unlock(&chip->ctl_lock);
	if (ret < 0)
		goto unlock;

	/* A confirmed device-rate change is a re-base epoch: the host reapplies
	 * the scene (rate-dependent DSP) and re-bases its per-run view. */
	chip->run_seq++;
	dev_dbg(&chip->dev->dev,
		"idle rate write: %u Hz confirmed %u Hz run_seq %u\n",
		rate, *confirmed, chip->run_seq);
unlock:
	mutex_unlock(&chip->mutex);
	return ret;
}

static int ff_pcm_open(struct snd_pcm_substream *subs)
{
	struct ff *chip = snd_pcm_substream_chip(subs);
	struct snd_pcm_runtime *rt = subs->runtime;
	int ret;

	if (READ_ONCE(chip->disconnected))
		return -ENODEV;

	rt->hw = ff_pcm_hw;
	ret = snd_pcm_hw_constraint_list(rt, 0, SNDRV_PCM_HW_PARAM_RATE,
					 &ff_rate_list);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_CHANNELS,
				  ff_rule_channels, subs,
				  SNDRV_PCM_HW_PARAM_RATE, -1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_RATE,
				  ff_rule_rate, subs,
				  SNDRV_PCM_HW_PARAM_CHANNELS, -1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_PERIOD_SIZE,
				  ff_rule_period, chip,
				  SNDRV_PCM_HW_PARAM_RATE, -1);
	if (ret < 0)
		return ret;
	ret = snd_pcm_hw_rule_add(rt, 0, SNDRV_PCM_HW_PARAM_BUFFER_SIZE,
				  ff_rule_buffer, chip,
				  SNDRV_PCM_HW_PARAM_RATE, -1);
	if (ret < 0)
		return ret;
	return snd_pcm_hw_constraint_msbits(rt, 0, 32, 24);
}

static int ff_pcm_close(struct snd_pcm_substream *subs)
{
	return 0;
}

static int ff_pcm_hw_params(struct snd_pcm_substream *subs,
			      struct snd_pcm_hw_params *params)
{
	struct ff *chip = snd_pcm_substream_chip(subs);
	unsigned int rate = params_rate(params);
	unsigned int self = 1 << subs->stream;
	int ret = 0;

	if (params_channels(params) > ff_max_channels(chip, subs->stream, rate))
		return -EINVAL;

	/* The period rule cannot express a stride; enforce the interrupt
	 * whole-URB period here, and the buffer minimum, for a host that
	 * bypasses the interval rules. */
	if (!chip->iso) {
		unsigned int urb = ff_urb_frames(chip, rate);
		unsigned int period = params_period_size(params);

		if (period < urb || period % urb)
			return -EINVAL;
	}
	if (params_buffer_size(params) < ff_buffer_min(chip, rate))
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
		ff_engine_stop(chip);
		if (chip->caps & FF_TCAP_SESSION_ON_RATE_CHANGE) {
			chip->session_reopen = true;
			chip->uc_session = true;
		}
		chip->prepared_mask &= ~self;
		chip->rate = rate;
		chip->clock_rate = rate;
		chip->mode = &chip->modes[ff_mode_index(rate)];
	}
	chip->hw_params_mask |= self;
unlock:
	mutex_unlock(&chip->mutex);
	return ret;
}

static int ff_pcm_hw_free(struct snd_pcm_substream *subs)
{
	struct ff *chip = snd_pcm_substream_chip(subs);
	unsigned int self = 1 << subs->stream;

	mutex_lock(&chip->mutex);
	chip->hw_params_mask &= ~self;
	chip->prepared_mask &= ~self;
	if (!chip->prepared_mask)
		ff_engine_stop(chip);
	mutex_unlock(&chip->mutex);
	return 0;
}

static int ff_pcm_prepare(struct snd_pcm_substream *subs)
{
	struct ff *chip = snd_pcm_substream_chip(subs);
	struct ff_stream *s = &chip->streams[subs->stream];
	int ret;

	mutex_lock(&chip->mutex);
	if (READ_ONCE(chip->engine_broken))
		ff_engine_stop(chip);
	ret = ff_engine_start(chip);
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
	s->out_prefill_pending = false;
	if (subs->stream == SNDRV_PCM_STREAM_PLAYBACK)
		s->out_counter_valid = false;
	/* Preparation never zeroed the ring; inherit the last session's
	 * (or the allocator's) bytes.  Poison so that a recorded zero can
	 * never be confused with never-written memory.  Safe amplitude in
	 * both formats. */
	if (ff_poison && subs->runtime && subs->runtime->dma_area)
		memset32((u32 *)subs->runtime->dma_area, 0x00005A5AU,
			 subs->runtime->dma_bytes / 4);
	snd_pcm_stream_unlock_irq(subs);
	return 0;
}

static int ff_pcm_trigger(struct snd_pcm_substream *subs, int cmd)
{
	struct ff *chip = snd_pcm_substream_chip(subs);
	struct ff_stream *s = &chip->streams[subs->stream];
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
		s->out_start_urbs = FF_START_GRACE_URBS;
		s->out_counter_valid = false;
		s->running = true;
		if (subs->stream == SNDRV_PCM_STREAM_PLAYBACK && prefill &&
		    chip->desc->ops->hw_mute) {
			unsigned int urb_ms =
				max(1u, chip->urb_frames * 1000 / chip->rate);

			s->out_prefill_pending = true;
			s->out_prefill_stable = 0;
			s->out_prefill_need =
				max(1u, FF_PREFILL_STABLE_MS / urb_ms);
			s->out_prefill_min =
				max(1u, FF_PREFILL_MIN_MS / urb_ms);
			s->out_prefill_max =
				max(s->out_prefill_min + 1,
				    FF_PREFILL_MAX_MS / urb_ms);
			s->out_prefill_bound = s->out_prefill_max;
			queue_work(system_unbound_wq, &chip->prefill_work);
		}
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
		 * be restarted in ff_resume(), refuse the resume so ALSA
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
		s->out_start_urbs = FF_START_GRACE_URBS;
		s->out_counter_valid = false;
		s->running = true;
		if (subs->stream == SNDRV_PCM_STREAM_PLAYBACK && prefill &&
		    chip->desc->ops->hw_mute) {
			unsigned int urb_ms =
				max(1u, chip->urb_frames * 1000 / chip->rate);

			s->out_prefill_pending = true;
			s->out_prefill_stable = 0;
			s->out_prefill_need =
				max(1u, FF_PREFILL_STABLE_MS / urb_ms);
			s->out_prefill_min =
				max(1u, FF_PREFILL_MIN_MS / urb_ms);
			s->out_prefill_max =
				max(s->out_prefill_min + 1,
				    FF_PREFILL_MAX_MS / urb_ms);
			s->out_prefill_bound = s->out_prefill_max;
			queue_work(system_unbound_wq, &chip->prefill_work);
		}
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
				if (ff_cadence_stop_mismatch(&chip->cad,
							       ff_programmed_clock(chip))) {
					atomic_inc(&chip->rate_mismatch);
					dev_warn_ratelimited(&chip->dev->dev,
							     "ISO cadence %u Hz disagrees with clock %u Hz\n",
							     chip->iso_cadence,
							     ff_programmed_clock(chip));
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

unsigned int ff_playback_inflight(struct ff *chip)
{
	struct ff_stream *s = &chip->streams[SNDRV_PCM_STREAM_PLAYBACK];
	unsigned int i, n = UINT_MAX;

	/* At 4x the channels are split over two independent endpoint pairs.
	 * The queue the host sees is one pair's, and the reported position is
	 * the pair that is behind (ff_stream_advance), so the backlog is the
	 * minimum across pairs; summing them would double-count it. */
	if (chip->iso || !chip->mode || chip->mode->neps < 2)
		return (unsigned int)atomic_read(&s->inflight);
	for (i = 0; i < chip->mode->neps; i++)
		n = min(n, (unsigned int)atomic_read(
			&chip->eps[SNDRV_PCM_STREAM_PLAYBACK][i].inflight));
	return n == UINT_MAX ? 0 : n;
}

static snd_pcm_uframes_t ff_pcm_pointer(struct snd_pcm_substream *subs)
{
	struct ff *chip = snd_pcm_substream_chip(subs);

	/* The pointer is the driver's read/write position.  runtime->delay
	 * is the queue between it and the device: for playback, the frames
	 * handed to USB but not yet completed.  Capture in-flight frames are
	 * not accounted yet, so the capture delay is the ring fill.  ALSA
	 * adds hw_avail to it, so snd_pcm_delay()/htimestamp see the real
	 * queue (audit R4). */
	subs->runtime->delay = subs->stream == SNDRV_PCM_STREAM_PLAYBACK ?
		ff_playback_inflight(chip) :
		atomic_read(&chip->streams[subs->stream].inflight);

	return chip->streams[subs->stream].hw_ptr %
	       subs->runtime->buffer_size;
}

static int ff_pcm_sync_stop(struct snd_pcm_substream *subs)
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

static const struct snd_pcm_ops ff_pcm_ops = {
	.open = ff_pcm_open,
	.close = ff_pcm_close,
	.hw_params = ff_pcm_hw_params,
	.hw_free = ff_pcm_hw_free,
	.prepare = ff_pcm_prepare,
	.trigger = ff_pcm_trigger,
	.pointer = ff_pcm_pointer,
	.sync_stop = ff_pcm_sync_stop,
};

static int ff_create_pcm(struct ff *chip)
{
	struct snd_pcm *pcm;
	int ret;

	ret = snd_pcm_new(chip->card, "Fireface", 0, 1, 1, &pcm);
	if (ret < 0)
		return ret;
	pcm->private_data = chip;
	strscpy(pcm->name, "Fireface", sizeof(pcm->name));
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_PLAYBACK, &ff_pcm_ops);
	snd_pcm_set_ops(pcm, SNDRV_PCM_STREAM_CAPTURE, &ff_pcm_ops);
	/*
	 * ff_suspend() decides per direction whether a PCM needs to enter
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

static int ff_alloc_urbs(struct ff *chip)
{
	int dir, e, i;

	chip->status_urb = usb_alloc_urb(0, GFP_KERNEL);
	chip->status_buf = kmalloc(FF_STATUS_LEN, GFP_KERNEL);
	if (!chip->status_urb || !chip->status_buf)
		return -ENOMEM;

	for (dir = 0; dir < 2; dir++) {
		for (e = 0; e < FF_MAX_EPS; e++) {
			struct ff_ep *ep = &chip->eps[dir][e];

			ep->chip = chip;
			ep->dir = dir;
			ep->index = e;
			atomic_set(&ep->inflight, 0);
			for (i = 0; i < chip->nurbs; i++) {
				struct ff_urb *u = &ep->urbs[i];

				u->ep = ep;
				u->urb = usb_alloc_urb(chip->iso ?
						       FF_MAX_ISO_PACKETS : 0,
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

static void ff_free_urbs(struct ff *chip)
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
		for (e = 0; e < FF_MAX_EPS; e++) {
			for (i = 0; i < FF_MAX_URBS; i++) {
				struct ff_urb *u = &chip->eps[dir][e].urbs[i];

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

static void ff_card_free(struct snd_card *card)
{
	struct ff *chip = card->private_data;

	cancel_work_sync(&chip->recover_work);
	cancel_work_sync(&chip->prefill_work);
	ff_hwdep_cleanup(chip);
	ff_midi_cleanup(chip);
	if (chip->status_intf) {
		usb_driver_release_interface(&ff_driver, chip->status_intf);
		chip->status_intf = NULL;
	}
	ff_free_urbs(chip);
	usb_put_dev(chip->dev);
	mutex_lock(&devices_mutex);
	clear_bit(chip->dev_idx, &devices_used);
	mutex_unlock(&devices_mutex);
}

static int ff_hw_setup(struct ff *chip)
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

static void ff_read_identity(struct ff *chip)
{
	__le32 rev = 0;

	if (ff_vendor_recv(chip, FF_REQ_REVISION, 0, 0, &rev,
			     sizeof(rev)) == 0)
		chip->firmware = (le32_to_cpu(rev) >> 16) & 0xff;
	if (chip->dev->serial)
		strscpy(chip->serial, chip->dev->serial, sizeof(chip->serial));
}

/* Derive the generic transport capabilities from the device table.  This is
 * the only place the model identity selects behaviour; the transport code
 * uses the capability bits.  Phase 2 removes the model name from the ABI. */
static u32 ff_caps_for_desc(const struct ff_device_desc *d)
{
	u32 caps = 0;

	switch (d->product) {
	case FF_PRODUCT_ID_A:
	case FF_PRODUCT_ID_C:
	case FF_PRODUCT_ID_B:
		caps |= FF_TCAP_SKIP_SAME_ALT |
			FF_TCAP_SESSION_BEFORE_URBS |
			FF_TCAP_SESSION_ON_RATE_CHANGE |
			FF_TCAP_METER_PAIR;
		if (d->transport == FF_XPORT_ISO)
			caps |= FF_TCAP_KILL_BOTH;
		else
			caps |= FF_TCAP_FIXED_URB;
		break;
	case FF_PRODUCT_ID_D:
		caps |= FF_TCAP_FLASH_GUARD |
			FF_TCAP_MUTE_ON_STOP |
			FF_TCAP_FRAME_LEAD |
			FF_TCAP_STATUS_RUNNING_ONLY |
			FF_TCAP_LEAD_HORIZON |
			FF_TCAP_KILL_BOTH |
			FF_TCAP_IF5_SELECT;
		if (d->transport == FF_XPORT_INTERRUPT)
			caps |= FF_TCAP_FIXED_URB;
		break;
	case FF_PRODUCT_ID_E:
		caps |= FF_TCAP_FLASH_GUARD |
			FF_TCAP_MUTE_ON_STOP |
			FF_TCAP_FRAME_LEAD |
			FF_TCAP_STATUS_RUNNING_ONLY |
			FF_TCAP_LEAD_HORIZON |
			FF_TCAP_MUTE_MASK_18BIT |
			FF_TCAP_KILL_BOTH;
		break;
	default:
		break;
	}
	return caps;
}


static int ff_probe(struct usb_interface *intf,
		      const struct usb_device_id *usb_id)
{
	struct usb_device *dev = interface_to_usbdev(intf);
	const struct ff_device_desc *desc =
		(const struct ff_device_desc *)usb_id->driver_info;
	struct snd_card *card;
	struct ff *chip;
	unsigned int iface_no = intf->cur_altsetting->desc.bInterfaceNumber;
	u32 caps;
	int dev_idx, ret;

	if (!desc)
		return -ENODEV;
	caps = ff_caps_for_desc(desc);
	if (iface_no != desc->iface)
		return -ENODEV;
	/* Never bind a class-compliant (UAC2) interface.  In CC mode the
	 * device must be left to snd-usb-audio; this driver is only for the
	 * vendor-specific RME personality. */
	if (intf->cur_altsetting->desc.bInterfaceClass !=
	    USB_CLASS_VENDOR_SPEC)
		return -ENODEV;

	/* One card per physical UCX: bind either IF0 or IF5, never both. */
	if (caps & FF_TCAP_IF5_SELECT &&
	    ((ucx_if5 && desc->transport != FF_XPORT_INTERRUPT) ||
	     (!ucx_if5 && desc->transport != FF_XPORT_ISO)))
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
	chip->iso = desc->transport == FF_XPORT_ISO;
	chip->caps = caps;
	chip->dev_idx = dev_idx;
	chip->audio_iface = desc->audio_iface;
	chip->out_ep = desc->out_ep;
	chip->in_ep = desc->in_ep;
	chip->modes = desc->modes;
	chip->debugfs = debugfs_create_dir(dev_name(&intf->dev), NULL);
	if (!IS_ERR_OR_NULL(chip->debugfs)) {
		debugfs_create_file("stats", 0444, chip->debugfs, chip,
				    &ff_stats_fops);
		debugfs_create_file("ringtrace", 0444, chip->debugfs, chip,
				    &ff_ringtrace_fops);
		debugfs_create_file("clock", 0444, chip->debugfs, chip,
				    &ff_clock_fops);
	}
	chip->ring_min_filled = UINT_MAX;
	mutex_init(&chip->mutex);
	mutex_init(&chip->ctl_lock);
	spin_lock_init(&chip->fb_lock);
	spin_lock_init(&chip->gate_lock);
	INIT_WORK(&chip->recover_work, ff_recover_work);
	INIT_WORK(&chip->prefill_work, ff_prefill_work);
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
	if (kfifo_alloc(&chip->event_fifo, FF_EVENT_FIFO_SIZE, GFP_KERNEL)) {
		ret = -ENOMEM;
		goto free_card;
	}
	card->private_free = ff_card_free;

	chip->nurbs = clamp(nurbs, 2u, (unsigned int)FF_MAX_URBS);
	chip->iso_packets = clamp(iso_packets, 1u,
				  (unsigned int)FF_MAX_ISO_PACKETS);
	{
		unsigned int short_frames = desc->product == FF_PRODUCT_ID_D ?
			ucx_if5_frames : uc_pc_frames;
		unsigned int base;

		if (chip->iso) {
			/* The ISO URB is iso_packets microframes; at 48 kHz
			 * one microframe carries six frames.  Keep the base
			 * exact so the envelope reports the real URB.
			 */
			base = chip->iso_packets * DIV_ROUND_UP(48000, 8000);
			chip->base_urb_frames = base;
		} else {
			base = (chip->caps & FF_TCAP_FIXED_URB) ?
				short_frames : urb_frames;
			chip->base_urb_frames = clamp(round_up(base, 8), 8u,
				(chip->caps & FF_TCAP_FIXED_URB) ? 1024u : 256u);
		}
	}
	/* The interrupt transports (UC PC and UCX IF5) keep the configured
	 * queue depth.  The reference queues two short URBs (32 frames at 1x);
	 * the 64-frame ISO floor would defeat that geometry.  The ISO queue
	 * is the configured nurbs at the chosen iso_packets.
	 */
	if (!chip->iso && !(chip->caps & FF_TCAP_FIXED_URB) &&
	    chip->nurbs * chip->base_urb_frames < FF_MIN_QUEUE_FRAMES) {
		chip->nurbs = DIV_ROUND_UP(FF_MIN_QUEUE_FRAMES,
					   chip->base_urb_frames);
		dev_dbg(&intf->dev, "using %u URBs to keep %u frames queued\n",
			chip->nurbs, FF_MIN_QUEUE_FRAMES);
	}
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
			unsigned int frames = chip->base_urb_frames << i;

			span = max(span, frames *
				   max(chip->modes[i].words,
				       ff_mode_out_words(&chip->modes[i])));
		}
		chip->urb_bytes_max = span * FF_SAMPLE_BYTES;
	}
	if (chip->iso) {
		const struct ff_mode *im;
		unsigned int k;

		for (k = 0; k < chip->desc->n_modes; k++) {
			im = &chip->modes[k];
			/* The feedback frames field is 5 bits and the packet
			 * arithmetic must stay inside it.
			 */
			WARN_ON(im->max_frames > 31);
			WARN_ON(im->neps != 1);
			WARN_ON(im->out_mps < im->max_frames *
				(im->words + 1) * FF_SAMPLE_BYTES);
			WARN_ON(im->in_mps < FF_ISO_IN_HEADER +
				im->max_frames * im->words *
				FF_SAMPLE_BYTES);
		}
	}
	chip->rate = 48000;
	chip->clock_rate = 48000;
	chip->mode = &chip->modes[0];
	dev_idx = -1;		/* released by ff_card_free() from now on */

	strscpy(card->driver, "FirefaceUSB", sizeof(card->driver));
	strscpy(card->shortname, "Fireface", sizeof(card->shortname));
	strscpy(card->mixername, "Fireface", sizeof(card->mixername));
	if (!id[chip->dev_idx])
		snd_card_set_id(card, "Fireface");

	ff_midi_init(chip);
	ret = ff_alloc_urbs(chip);
	if (ret < 0)
		goto free_card;

	if (desc->status_iface != FF_IFACE_NONE) {
		chip->status_intf = usb_ifnum_to_if(dev, desc->status_iface);
		if (chip->status_intf &&
		    usb_driver_claim_interface(&ff_driver, chip->status_intf,
					       NULL) < 0) {
			dev_warn(&intf->dev,
				 "status interface busy, no underrun recovery\n");
			chip->status_intf = NULL;
		}
	}

	ff_read_identity(chip);
	chip->s32_samples = ff_wire_s32(chip);
	dev_info(&intf->dev, "firmware %u, %s samples on the wire\n",
		 chip->firmware, chip->s32_samples ? "S32" : "float");
	{
		__le32 st = 0;

		if (ff_vendor_recv(chip, FF_REQ_STATUS, 0, 0, &st,
				     sizeof(st)) == 0)
			dev_info(&intf->dev,
				 "probe status 0x%08x rate index %u (device state before any write)\n",
				 le32_to_cpu(st), le32_to_cpu(st) >> 24);
	}
	snprintf(card->longname, sizeof(card->longname),
		 "RME Fireface (%s transport, firmware %u) at %s",
		 chip->iso ? "ISO" : "interrupt",
		 chip->firmware, dev_name(&dev->dev));

	ret = ff_hw_setup(chip);
	if (ret < 0) {
		dev_err(&intf->dev, "hardware init failed: %d\n", ret);
		goto free_card;
	}
	ret = ff_create_pcm(chip);
	if (ret < 0)
		goto free_card;
	ret = ff_hwdep_create(chip);
	if (ret < 0)
		goto free_card;
	ret = ff_midi_create(chip);
	if (ret < 0)
		dev_warn(&intf->dev, "no MIDI port: %d\n", ret);
	ret = ff_transport_setup(chip);
	if (ret < 0)
		dev_warn(&intf->dev, "raw control transports unavailable: %d\n",
			 ret);
	ret = snd_card_register(card);
	if (ret < 0)
		goto free_card;

	usb_set_intfdata(intf, chip);
	if (chip->status_intf && ff_status_submit(chip) < 0)
		dev_warn(&intf->dev, "cannot watch the status endpoint\n");
	dev_info(&intf->dev, "%s, card %d, %u frames x %u URBs\n",
		 card->longname, card->number, chip->base_urb_frames,
		 chip->nurbs);
	return 0;

free_card:
	if (chip->status_intf) {
		usb_driver_release_interface(&ff_driver, chip->status_intf);
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

static void ff_disconnect(struct usb_interface *intf)
{
	struct ff *chip = usb_get_intfdata(intf);

	if (!chip)
		return;
	usb_set_intfdata(intf, NULL);

	/* Mark the card dead before ALSA tears it down.  An hwdep ioctl that
	 * starts after this point returns -ENODEV instead of touching a
	 * runtime that is being dismantled. */
	WRITE_ONCE(chip->disconnected, true);
	snd_card_disconnect(chip->card);
	debugfs_remove_recursive(chip->debugfs);
	chip->debugfs = NULL;
	mutex_lock(&chip->mutex);
	WRITE_ONCE(chip->engine_running, false);
	ff_kill_urbs(chip, true);
	mutex_unlock(&chip->mutex);
	cancel_work_sync(&chip->recover_work);
	cancel_work_sync(&chip->prefill_work);
	ff_hwdep_quiesce(chip);
	ff_midi_cleanup(chip);
	if (chip->status_intf) {
		usb_driver_release_interface(&ff_driver, chip->status_intf);
		chip->status_intf = NULL;
	}
	snd_card_free_when_closed(chip->card);
}

static int ff_suspend(struct usb_interface *intf, pm_message_t message)
{
	struct ff *chip = usb_get_intfdata(intf);
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
	cancel_work_sync(&chip->prefill_work);
	mutex_lock(&chip->mutex);
	ff_engine_stop(chip);
	mutex_unlock(&chip->mutex);
	ff_midi_suspend(chip);
	ff_transport_suspend(chip);
	return 0;
}

static int ff_resume_common(struct usb_interface *intf, bool reset)
{
	struct ff *chip = usb_get_intfdata(intf);
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
		if (chip->caps & FF_TCAP_SESSION_ON_RATE_CHANGE) {
			chip->uc_session = true;
			chip->session_reopen = true;
		}
		chip->cur_alt = 0;
		mutex_unlock(&chip->mutex);
	}

	mutex_lock(&chip->mutex);
	ret = ff_hw_setup(chip);
	if (ret == 0 && chip->prepared_mask && !chip->engine_running)
		engine_ret = ff_engine_start(chip);
	mutex_unlock(&chip->mutex);

	if (ret == 0) {
		int transport_ret = ff_transport_setup(chip);

		if (transport_ret < 0)
			dev_warn(&chip->dev->dev,
				 "resume: transport setup failed: %d\n",
				 transport_ret);
	}
	if (engine_ret < 0)
		dev_warn(&chip->dev->dev,
			 "resume: engine restart failed: %d\n", engine_ret);

	ff_midi_resume(chip);
	dev_dbg(&chip->dev->dev,
		"resume: reset=%d prepared=0x%x running=0x%x engine=%d ret=%d\n",
		reset, chip->prepared_mask, chip->resume_mask,
		READ_ONCE(chip->engine_running), ret);
	snd_power_change_state(chip->card, SNDRV_CTL_POWER_D0);
	return ret;
}

static int ff_resume(struct usb_interface *intf)
{
	return ff_resume_common(intf, false);
}

static int ff_reset_resume(struct usb_interface *intf)
{
	return ff_resume_common(intf, true);
}

static struct usb_driver ff_driver = {
	.name = "snd-fireface-uc",
	.probe = ff_probe,
	.disconnect = ff_disconnect,
	.suspend = ff_suspend,
	.resume = ff_resume,
	.reset_resume = ff_reset_resume,
	.id_table = ff_ids,
	.supports_autosuspend = 0,
};

module_usb_driver(ff_driver);

MODULE_DESCRIPTION("RME Fireface UC/UCX USB audio transport (PCM + hwdep)");
MODULE_LICENSE("GPL");
