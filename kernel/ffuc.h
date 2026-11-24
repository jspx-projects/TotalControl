/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * RME Fireface USB audio driver - core definitions.
 *
 * Design (2026-10-25 refactor)
 * ----------------------------
 * The kernel module is intentionally a *transport* driver, not a mixer:
 *
 *   - it enumerates the USB device, selects the streaming alternate setting,
 *     runs the URB engine and exposes the ALSA PCM device;
 *   - it runs the device's raw control transports (EP0 vendor requests and,
 *     where the device has one, the interface-1 bulk register/meter channel);
 *   - it exposes all of that through one snd-hwdep node per card;
 *   - it contains a table of per-device descriptors so a single module binds
 *     to several RME models and to several units at once.
 *
 * Everything that is policy or a device model - matrix routing, faders, pan,
 * preamps, EQ/dynamics, clock preferences, meters, scenes - is implemented
 * in userspace (TotalControl) on top of the hwdep ABI in ffuc-uapi.h.  Do not add
 * model-specific ALSA controls here; add a userspace profile instead.
 *
 * Transport summary
 * -----------------
 * Fireface UC PC personality (0424:3fc5 / 2a39:3fc5 / 2a39:3fce,
 * interface 0):
 *   interrupt endpoints 0x01 OUT / 0x82 IN (0x03/0x84 at 4x), interleaved
 *   float32 frames, no header.  The status ticker on interface 5 (0x8b)
 *   reports front-panel events and the output-latch bit.
 *
 * Fireface UC Mac personality (0424:3fc6 / 2a39:3fc6):
 *   isochronous endpoints 0x01/0x82; IN packets are a 4-byte header plus
 *   planar frames, OUT packets are frames with a per-frame lead word.
 *
 * Fireface UCX (0424:3fc9 / 2a39:3fc9):
 *   interface 0 has the ISO endpoints 0x0e OUT / 0x8f IN; interface 5 has
 *   the Windows interrupt endpoints 0x01/0x82.  The module parameter
 *   ucx_if5 chooses which personality binds (default ISO IF0).
 *
 * Babyface-classic (0424:3fc7 / 2a39:3fc7):
 *   UCX-family layout.  Interface 0 has the ISO endpoints 0x03 OUT /
 *   0x84 IN with the UCX packet format; capture carries 12/8/6 words and
 *   playback 14/10/8 words at 1x/2x/4x.  Interface 1 bulk IN 0x85 streams
 *   the DSP meter packets.  USB-MIDI is on interface 2.
 */
#ifndef FFUC_H
#define FFUC_H

#include <linux/kfifo.h>
#include <linux/mutex.h>
#include <linux/poll.h>
#include <linux/spinlock.h>
#include <linux/usb.h>
#include <linux/wait.h>
#include <linux/workqueue.h>
#include <sound/core.h>
#include <sound/hwdep.h>
#include <sound/pcm.h>
#include <sound/rawmidi.h>

#include "ffuc-uapi.h"
#include "ffuc-cadence.h"

#define FFUC_VENDOR_SMSC	0x0424
#define FFUC_VENDOR_RME		0x2a39
#define FFUC_PRODUCT_UC_PC	0x3fc5
#define FFUC_PRODUCT_UC_PC_RME	0x3fc5
#define FFUC_PRODUCT_UC_WIN_RME	0x3fce
#define FFUC_PRODUCT_UC_MAC	0x3fc6
#define FFUC_PRODUCT_UCX	0x3fc9
#define FFUC_PRODUCT_BABYFACE_CLASSIC	0x3fc7

#define FFUC_IFACE_NONE		0xff

#define FFUC_STATUS_LEN		4
#define FFUC_STATUS_OUT_LATCHED	0x80
#define FFUC_STATUS_GRACE_MS	20
/* UCX IF5 settings-group settle, measured in the E2b connect capture. */
#define FFUC_UCX_IF5_SETTLE_MS	100
#define FFUC_RESTART_SETTLE_MS	250	/* UC: minimum engine stop -> start gap */

#define FFUC_MAX_EPS		2	/* endpoints per direction (alt 3) */
#define FFUC_MAX_URBS		32
#define FFUC_MAX_CHANNELS	18
#define FFUC_MAX_RATE		768000
#define FFUC_SAMPLE_BYTES	4
#define FFUC_MAX_URB_ERRORS	16
#define FFUC_MIN_QUEUE_FRAMES	64
#define FFUC_START_GRACE_URBS	4	/* starved URBs tolerated after start */
/* Startup prefill gate: the playback outputs stay muted from trigger for a
 * minimum window, then until delivery is stable, with a hard bound.
 * PipeWire's startup prefill is silence written into the ring, so a
 * fill-level threshold cannot gate real audio; delivery stability can.
 * Trace evidence (desk UCX, sd0): starvation clusters span 0-12 ms in
 * timer mode and 0-30 ms in IRQ mode, with gaps between clusters — a
 * stability-only gate clears between clusters.  The minimum window covers
 * the whole transient (2x the worst observed); the bound keeps a dead
 * host from sticking muted.  Milliseconds, not URBs: an interrupt URB is
 * ~21 ms at 48 kHz, an ISO URB is 1 ms at every rate. */
#define FFUC_PREFILL_MIN_MS	64
#define FFUC_PREFILL_MAX_MS	256
#define FFUC_PREFILL_STABLE_MS	4
#define FFUC_MIDI_IN_URBS	2	/* bulk IN URBs kept queued */
#define FFUC_MIDI_PORTS		2	/* logical ports on the endpoint pair */
#define FFUC_CTL_TIMEOUT	1000	/* ms */

/* ISO ("Mac"/IF0) transport. */
/* After the host stops polling, the device resets its capture stream
 * counter.  Polling before the reset completes catches the previous
 * timeline's tail mixed with the reset: position-0 placeholder packets,
 * a position jump, and an occasional error microframe.  The start margin
 * places the first URB after the reset, so the run starts on a clean
 * zero-based timeline.  Trace-bounded on UCX firmware 49: 2 ms is too
 * short, 20 ms is clean across the rate families.  Linux xHCI schedules
 * a new isochronous stream at the current frame index and ignores a
 * requested future start frame, so the margin is a submission delay. */
#define FFUC_ISO_START_MARGIN_MS	20
#define FFUC_MAX_ISO_PACKETS	8	/* per URB, one per 125 us microframe */
#define FFUC_ISO_IN_HEADER	4
#define FFUC_ISO_COUNTER_MOD	0x2000
#define FFUC_FB_SIZE		1024
#define FFUC_ISO_FRAME_MASK	0x7ff

struct ffuc;
unsigned int ffuc_iso_wrap(struct ffuc *chip);
unsigned int ffuc_rate_family(unsigned int rate);

#define FFUC_STATE_BUF_SIZE	1024
#define FFUC_EVENT_FIFO_SIZE	(64 * 1024)
#define FFUC_METER_PAIR_MAX	648
/* Backpressure marks for the UC heartbeat feed, in events.  The high mark
 * covers one slow 15 Hz consumer period with jitter; the low mark gives
 * hysteresis. */
#define FFUC_METER_HIGH_WATER	16
#define FFUC_METER_LOW_WATER	4
#define FFUC_EVENT_BUF_SIZE	(sizeof(struct ffuc_hwdep_event_hdr) + \
				 FFUC_EVENT_MAX)

/* Vendor request numbers, shared by the UC and UCX.  The meaning of some
 * numbers differs between models; see docs/PROTOCOL-COMPLETE.md.
 */
#define FFUC_REQ_SETTINGS	0x10
#define FFUC_REQ_STATUS		0x11
#define FFUC_REQ_MIXER		0x12
#define FFUC_REQ_MUTE		0x13
#define FFUC_REQ_UNMUTE		0x14
#define FFUC_REQ_LOOPBACK	0x15
#define FFUC_REQ_CHAN_INIT	0x16
#define FFUC_REQ_LEVELS		0x17
#define FFUC_REQ_PGA		0x1a
#define FFUC_REQ_DDS		0x1b
#define FFUC_REQ_REVISION	0x1c
#define FFUC_REQ_ENABLE		0x20

/* wIndex of the 0x10 options commit (1487): the mask of the option bits
 * (word clock termination, clock source, word clock single speed, SPDIF
 * and optical output format). */
#define FFUC_OPTIONS_MASK	0x05cf

/* UCX/Windows path: request 16/19/20/21/22/23/27/29 and the status reads
 * 28/30/31.  The numbers are documented with the protocol, not repeated
 * here. */
#define FFUC_REQ_UCX_INIT22	0x16
#define FFUC_REQ_UCX_MUTE	0x13
#define FFUC_REQ_UCX_UNMUTE	0x14
#define FFUC_REQ_UCX_LOOPBACK	0x15
#define FFUC_REQ_UCX_STREAM29	0x1d
#define FFUC_REQ_UCX_FIRMWARE	0x1c
#define FFUC_REQ_UCX_SERIAL_LO	0x1e
#define FFUC_REQ_UCX_SERIAL_HI	0x1f

/* quirks */
#define FFUC_QUIRK_ALT_RESELECT		BIT(0)	/* alt 0 then alt N at start */
#define FFUC_QUIRK_STATUS_LATCH		BIT(2)	/* status byte 3 bit 7 */
#define FFUC_QUIRK_IF5_PRE_START	BIT(3)	/* 0x10 0x8000 + 0x1d before audio */
#define FFUC_QUIRK_IF5_PLAYBACK_MASK	BIT(4)	/* req 19/20 channel mask */
/* Windows session timing: the stop->start settle and the close->rate and
 * close->trio gaps, measured on the UC_WIN_RME unit (captures A2/A2b).
 * The session words themselves are model-wide; only the timing is a
 * Windows-personality property. */
#define FFUC_QUIRK_WIN_SESSION		BIT(6)

struct ffuc;

/* One USB bandwidth class (alternate setting). */
struct ffuc_mode {
	unsigned int alt;
	unsigned int neps;		/* endpoints per direction */
	unsigned int words;		/* channels (32-bit words) per endpoint */
	unsigned int out_words;		/* playback words; 0 = same as words */
	unsigned int pkt_frames;	/* interrupt: frames in one max packet */
	/* ISO only */
	unsigned int in_mps;
	unsigned int out_mps;
	unsigned int max_frames;
};

static inline unsigned int ffuc_mode_out_words(const struct ffuc_mode *m)
{
	return m->out_words ? m->out_words : m->words;
}

struct ffuc_urb {
	struct urb *urb;
	struct ffuc_ep *ep;
	void *buf;
	dma_addr_t dma;
	unsigned int frames;	/* playback frames in the last fill */
};

struct ffuc_ep {
	struct ffuc *chip;
	int dir;
	unsigned int index;
	int next_start;
	unsigned int buf_size;      /* allocated bytes for one URB */
	int last_start;             /* diagnostic: previous start_frame */
	struct ffuc_urb urbs[FFUC_MAX_URBS];
};

/* Ring-fill model trace: one record per playback URB completion while
 * the stream runs.  Diagnostic only; the audio path is unchanged. */
#define FFUC_RING_TRACE		4096
#define FFUC_RING_F_UNDERRUN	0x1	/* dry against the published status hw_ptr */
#define FFUC_RING_F_STARVED	0x2	/* dry against the driver read position */
#define FFUC_RING_F_MISSED	0x4	/* a full period passed without a write */
#define FFUC_RING_F_BEHIND	0x8	/* appl_ptr is behind the read position */

struct ffuc_ring_rec {
	u64 ktime_ns;
	u64 appl;		/* control->appl_ptr at refill time */
	u64 hw;			/* ring position this completion reads */
	u32 status_hw;		/* low 32 bits of the published status hw_ptr */
	u32 filled;		/* (appl - hw) mod boundary */
	u32 total;		/* frames this URB sends */
	u32 flags;		/* FFUC_RING_F_* */
};

struct ffuc_stream {
	struct snd_pcm_substream *substream;
	bool running;
	u32 out_counter;
	bool out_counter_valid;
	snd_pcm_uframes_t pos[FFUC_MAX_EPS];
	snd_pcm_uframes_t hw_ptr;
	snd_pcm_uframes_t period;
	/* Period index at this stream's last period signal.  Reset at
	 * prepare and trigger; updated at every signal.  Compared against
	 * `period` to detect a signal that announces no completed period
	 * (a delta-0 call). */
	snd_pcm_uframes_t signalled;
	/* Consecutive wakes withheld while this direction had a completed
	 * period.  Bounds the gate's added latency to about one URB at any
	 * period size; reset on every signal and at stream start. */
	u32 deferrals;
	/* Consecutive starved playback completions.  The first starved URB
	 * of an episode sends silence and is counted; the second consecutive
	 * one stops the stream as an XRUN.  Reset at prepare and start. */
	unsigned int out_starve_streak;
	/* Start grace: the first URBs after trigger start send silence on
	 * starvation without stopping, so a stream-start prefill shortfall
	 * (rate transition, renegotiation) recovers without an XRUN. */
	unsigned int out_start_urbs;
	/* Startup prefill gate (playback): armed at trigger, the outputs
	 * stay muted for out_prefill_min URBs, then until
	 * out_prefill_need consecutive non-starved anchored URBs, bounded
	 * by out_prefill_max.  While armed, starvation sends silence
	 * without counting an underrun or arming the XRUN stop: the muted
	 * transient is intended startup behaviour.  The mute write runs in
	 * prefill_work (sleeping USB transfer). */
	bool out_prefill_pending;
	unsigned int out_prefill_stable;
	unsigned int out_prefill_need;
	unsigned int out_prefill_min;
	unsigned int out_prefill_max;
	unsigned int out_prefill_bound;
	/* Playback frames handed to USB but not yet completed: the real
	 * in-flight queue behind hw_ptr.  Reported as runtime->delay in
	 * .pointer (R4).  Engine-scoped: added at every playback submit
	 * (including the engine-start submits), subtracted at every
	 * completion, rebased only after the synchronous kill in
	 * engine_stop.  PCM prepare/trigger do not reset it: the URB queue
	 * survives stream stops while the engine runs.  Atomic: submits and
	 * completions run in different contexts. */
	atomic_t inflight;
};

/* Per-model core callbacks.  They may touch the device transport but must
 * not create ALSA controls or implement mixer policy. */
struct ffuc_ops {
	int (*init)(struct ffuc *chip);
	int (*set_rate)(struct ffuc *chip, unsigned int rate);
	int (*stream_start)(struct ffuc *chip, unsigned int channels);
	int (*stream_stop)(struct ffuc *chip, unsigned int channels);
	int (*hw_mute)(struct ffuc *chip, bool mute);
};

/* One run of 0x16 channel-initialization writes.  The slot ranges differ
 * between device models and personalities, so they are data in the device
 * descriptor.  The wValue (channel sample format) is derived from the wire
 * format at init time, not stored here. */
struct ffuc_init_range {
	u16 first;
	u16 count;
};

struct ffuc_device_desc {
	const char *name;		/* "Fireface UC" */
	const char *shortname;
	const char *card_id;
	u16 vendor;
	u16 product;
	u8 iface;
	u8 transport;			/* enum ffuc_transport */
	u32 quirks;
	u32 model;			/* enum ffuc_model */
	const struct ffuc_mode *modes;
	unsigned int n_modes;
	u8 audio_iface;
	u8 out_ep;
	u8 in_ep;
	u8 status_iface;
	u8 status_ep;
	u8 status_bulk;		/* 1 = bulk IN, 0 = interrupt IN */
	u8 state_iface;
	u8 state_out_ep;
	u8 state_in_ep;
	u8 meter_in_ep;
	u8 midi_iface;
	u8 midi_out_ep;
	u8 midi_in_ep;
	unsigned int midi_ports;	/* logical ports on the MIDI endpoint pair */
	unsigned int max_channels;	/* capture channels */
	unsigned int max_playback;	/* playback channels; 0 = max_channels */
	unsigned int max_rate;
	unsigned int meter_len;
	const struct ffuc_init_range *chan_init;
	unsigned int n_chan_init;
	const struct ffuc_ops *ops;
};

static inline unsigned int
ffuc_desc_playback(const struct ffuc_device_desc *d)
{
	return d->max_playback ? d->max_playback : d->max_channels;
}

/* USB-MIDI output parser state, kept across rawmidi chunks. */
struct ffuc_midi_tx {
	u8 status;
	u8 msg[2];
	unsigned int msg_len;
	u8 sysex[3];
	unsigned int sysex_len;
	bool in_sysex;
};

/* One logical MIDI port carried on the shared endpoint pair. */
struct ffuc_midi_port {
	struct ffuc *chip;
	unsigned int index;
	struct snd_rawmidi *rmidi;
	struct snd_rawmidi_substream *in_sub;
	struct snd_rawmidi_substream *out_sub;
	struct delayed_work out_work;
	struct ffuc_midi_tx tx;
	u8 *out_buf;		/* bulk transfer buffer, must be DMA-able */
	bool out_running;
};

struct ffuc {
	struct usb_device *dev;
	struct usb_interface *intf;
	struct snd_card *card;
	struct snd_pcm *pcm;
	struct snd_hwdep *hwdep;

	const struct ffuc_device_desc *desc;
	unsigned int model;
	bool iso;
	bool ucx;
	unsigned int audio_iface;
	unsigned int cur_alt;		/* current streaming alternate setting */
	u8 out_ep, in_ep;
	const struct ffuc_mode *modes;

	struct mutex mutex;		/* engine/rate state */
	struct mutex ctl_lock;		/* serialises EP0/bulk control writes */
	int dev_idx;
	bool disconnected;

	/* engine */
	bool engine_running;
	bool engine_broken;
	/* UC: the next engine stop/start is a rate change and needs the
	 * Windows session-close/open words.  Not used on every start. */
	bool session_reopen;
	/* UC: once a live rate change has happened, every later stream
	 * open/close needs the Windows session trio/close word.  The initial
	 * 48 kHz startup stays untouched. */
	bool uc_session;
	unsigned int hw_params_mask;
	unsigned int prepared_mask;
	/* Directions that were running when the system suspended. */
	unsigned int resume_mask;
	unsigned int rate;
	/* The programmed clock reference (Hz): the nominal rate scaled by the
	 * pitch the host writes through the DDS divisor. The cadence judge
	 * compares the measured clock against this, so a legitimate varispeed
	 * offset is not a rate mismatch. */
	unsigned int clock_rate;
	const struct ffuc_mode *mode;
	unsigned int urb_frames;
	unsigned int base_urb_frames;
	unsigned int nurbs;
	size_t urb_bytes_max;
	atomic_t urb_errors;

	/* ISO implicit feedback */
	unsigned int iso_packets;
	spinlock_t fb_lock;
	u32 fb[FFUC_FB_SIZE];
	u64 in_pkts, out_pkts;
	u32 iso_frames_hist[32];
	/* Cadence convergence for the stop verdict (ffuc-cadence.h). */
	struct ffuc_cadence cad;
	unsigned int in_end;
	/* The device start ramp's position resync was seen this run: the
	 * first position discontinuity re-seeds the expectation instead of
	 * counting (ffuc_iso_capture_urb).  Reset at engine setup. */
	bool in_start_resync;
	unsigned int fb_acc;
	/* Nominal-cadence accumulator for the OUT priming window: carries
	 * the fractional frames per packet (rate/8000) so the priming lead
	 * words advance at the exact nominal rate.  Under fb_lock. */
	unsigned int out_prime_acc;
	unsigned int fb_delay;
	unsigned int out_lead;
	bool iso_frames_ok;

	/* Device timestamp (Stage 1): the start-frame anchor maps the
	 * running IN packet index to host nanoseconds (for high-speed,
	 * urb->start_frame advances one unit per ISO packet: 125 us
	 * microframes), and the sample is the unwrapped device position at
	 * a packet end paired with the host time of the next packet
	 * boundary.  All fields in this block are under fb_lock. */
	u64 ts_anchor_ns;      /* sof_ns(f) = ts_anchor_ns + f * 125000 */
	bool ts_anchor_valid;
	u64 ts_frames;         /* IN frame index of the next completion */
	u64 in_total;          /* unwrapped count model, whole frames */
	u64 in_field;          /* unwrapped field position (jump-carrying) */
	bool in_total_valid;
	u64 ts_ns;             /* latest sample: host time */
	u64 ts_pos;            /* latest sample: unwrapped position */
	u32 ts_phase;          /* latest sample: raw phase field */
	u64 ts_frac_q32;       /* phase/tps in Q32 frames (exact position) */
	u64 ts_tps_q32;        /* ticks per sample, Q32 (per rate) */
	s64 ts_seed_span;      /* seed diagnostics: packets rewound */
	u64 ts_seed_pos_q32;   /* seed diagnostics: position at the slot */
	u64 ts_seed_in;        /* seed diagnostics: in_pkts at the seed */
	u64 ts_seed_out;       /* seed diagnostics: out_pkts at the seed */
	u32 ts_seed_in_end;    /* seed diagnostics: in_end at the seed */
	u32 ts_seed_counter;   /* seed diagnostics: out_counter at the seed */
	bool ts_pos_valid;
	u64 ts_win_ns;         /* rate_meas window start */
	u64 ts_win_pos;
	unsigned int ts_rate_meas; /* windowed device rate, Hz */

	/* status ticker (core output-latch watchdog) */
	struct usb_interface *status_intf;
	struct urb *status_urb;
	u8 *status_buf;
	bool status_active;
	unsigned long status_armed_at;
	struct work_struct recover_work;
	unsigned int recoveries;
	struct work_struct prefill_work;
	atomic_t out_prefills;		/* gated startups that unmuted stable */
	atomic_t out_prefill_timeouts;	/* gated startups hit the bound */
	/* Engine-run epoch: increments on every engine start.  The per-run
	 * diagnostics reset with it; the host re-bases its counter view. */
	u32 run_seq;
	u8 last_status[FFUC_STATUS_LEN];
	bool out_latched;

	/* IF1 raw state/meter transports (owned by ffuc-hwdep.c) */
	struct usb_interface *state_intf;
	struct urb *state_urb;
	u8 *state_buf;
	dma_addr_t state_dma;
	struct urb *meter_urb;
	u8 *meter_buf;
	dma_addr_t meter_dma;
	bool state_active;
	bool transport_works_init;
	struct delayed_work state_work;
	struct delayed_work meter_work;
	atomic_t hwdep_users;
	atomic_t meter_urbs;
	/* One meter heartbeat under assembly: the RMS page then the peak
	 * page.  Emitted as one event, so a reader can never see a half. */
	u8 meter_pair[FFUC_METER_PAIR_MAX];
	unsigned int meter_pair_len;
	/* The device's page phase is not anchored for this session (fresh
	 * probe, a failed drain, or a pair without a peak half).  Pairs the
	 * record-table shape cannot decide carry the phase-lost flag until
	 * one pair decides it. */
	bool meter_phase_lost;
	/* Pair pump state for the reader backpressure. */
	bool meter_paused;
	u32 meter_pairs;
	/* Pairs not emitted to restore the RMS-then-peak order. */
	u32 meter_realigns;
	atomic_t state_urbs;

	/* USB-MIDI streaming interface (bulk OUT / bulk IN) */
	struct usb_interface *midi_intf;
	struct urb *midi_in_urb[FFUC_MIDI_IN_URBS];
	u8 *midi_in_buf[FFUC_MIDI_IN_URBS];
	atomic_t midi_in_users;
	bool midi_in_running;
	unsigned int n_midi_ports;
	struct ffuc_midi_port midi_port[FFUC_MIDI_PORTS];

	/* hwdep events */
	struct kfifo event_fifo;
	spinlock_t event_lock;
	/* Serializes the duplex period-gate decision: URB completions of
	 * the two directions can run concurrently on different CPUs and
	 * share the deferral counters.
	 */
	spinlock_t gate_lock;
	wait_queue_head_t event_wait;
	atomic_t event_seq;
	atomic_t events_dropped;

	/* transport health */
	struct dentry *debugfs;
	atomic_t out_urbs;
	atomic_t out_gaps;
	atomic_t out_shorts;
	atomic_t out_status_errors;
	atomic_t in_urbs;
	atomic_t in_gaps;
	atomic_t in_shorts;
	atomic_t in_status_errors;
	/* ISO per-packet failure split and header cross-checks. */
	atomic_t in_status_err;
	atomic_t in_short;
	atomic_t in_misaligned;
	atomic_t in_frames_mismatch;
	atomic_t in_pos_gap;
	atomic_t in_anchor_resync; /* start-frame anchor discontinuity */
	atomic_t out_capacity_err;
	atomic_t out_pkt_len_err;
	atomic_t out_fb_priming;
	atomic_t out_fb_ahead;
	atomic_t out_fb_stale;
	atomic_t out_fb_nominal;
	atomic_t fb_only_urbs;
	atomic_t in_hdr_only;      /* status 0, header-only packet (0 frames) */
	atomic_t out_pkt_status;   /* this run's per-packet completions with
				    * non-zero status; setup clears the reused
				    * URB descriptors so a previous run's
				    * cancelled deliveries never count here */
	int out_lag_min, out_lag_max;   /* (in_end - out_counter) mod wrap */
	atomic_t out_resync;
	atomic_t rate_mismatch;   /* cadence disagrees with chip->rate */
	atomic_t pitch_clamps;    /* DDS writes clamped to the pitch envelope */
	atomic_t in_frames_overflow; /* packet frames above the top bin */
	atomic_t pcm_prepare_err;
	atomic_t pcm_trigger_err;
	atomic_t signal_deferred;
	atomic_t signal_overdue;
	u32 max_pending;
	unsigned int iso_cadence; /* measured device clock, Hz */
	unsigned int rate_mismatch_windows; /* consecutive bad windows */
	u64 lag_n, lag_sx, lag_sy, lag_sxx, lag_sxy;
	s64 lag_syy;
	/* q48 instrumentation: ring pressure and core-pointer coherence.
	 * in_overruns: capture completions that overwrote unread frames.
	 * out_underruns: playback completions that sent non-written data.
	 * in_bad_length: status-0 IN URBs with an unaligned or overlong
	 *   payload.  hw_ptr_mismatch: snd_pcm_period_elapsed messages
	 *   where the core pointer diverged from the driver pointer. */
	atomic_t in_overruns;
	atomic_t out_underruns;
	atomic_t in_bad_length;
	atomic_t hw_ptr_mismatch;
	atomic_t out_bad_length;
	atomic_t delta_zero;
	unsigned int hw_ptr_logged;
	/* Ring-fill diagnostics, reset at engine start.  ring_head is
	 * monotonic across runs so the trace keeps its history. */
	struct ffuc_ring_rec ring_trace[FFUC_RING_TRACE];
	u32 ring_head;
	u64 ring_last_appl;
	u64 ring_last_appl_ts;
	u64 ring_prev_ts;
	u64 ring_max_stall_ns;
	unsigned int ring_min_filled;
	atomic_t ring_missed;
	atomic_t ring_starved;
	ktime_t last_out_complete;
	unsigned long engine_stopped_at;	/* jiffies; 0 = never stopped */
	ktime_t last_in_complete;

	/* identity */
	u32 firmware;
	bool s32_samples;	/* resolved wire format, see ffuc_wire_s32() */
	char serial[32];

	struct ffuc_ep eps[2][FFUC_MAX_EPS];
	struct ffuc_stream streams[2];
};

/* ffuc.c */
int ffuc_vendor_send(struct ffuc *chip, u8 req, u16 val, u16 idx,
		     const void *data, u16 len);
int ffuc_vendor_recv(struct ffuc *chip, u8 req, u16 val, u16 idx,
		     void *data, u16 len);
int ffuc_reg_send(struct ffuc *chip, const struct ffuc_hwdep_word *words,
		  unsigned int count);
int ffuc_claim_interface(struct usb_interface *intf);
void ffuc_release_interface(struct usb_interface *intf);
unsigned int ffuc_rate_from_index(unsigned int idx);
void ffuc_query_envelope(struct ffuc *chip, struct ffuc_hwdep_envelope *env);

/* ffuc-hwdep.c */
void ffuc_event_put(struct ffuc *chip, u32 type, const void *data, u32 size,
		    u32 flags);
int ffuc_hwdep_create(struct ffuc *chip);
void ffuc_hwdep_cleanup(struct ffuc *chip);
void ffuc_hwdep_quiesce(struct ffuc *chip);
int ffuc_transport_setup(struct ffuc *chip);
int ffuc_transport_start(struct ffuc *chip);
void ffuc_transport_stop(struct ffuc *chip);

/* ffuc-midi.c */
void ffuc_midi_init(struct ffuc *chip);
int ffuc_midi_create(struct ffuc *chip);
void ffuc_midi_cleanup(struct ffuc *chip);
void ffuc_midi_suspend(struct ffuc *chip);
void ffuc_midi_resume(struct ffuc *chip);
void ffuc_transport_suspend(struct ffuc *chip);

#endif /* FFUC_H */
