/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * RME Fireface USB - userspace ABI for the snd-hwdep control transport.
 *
 * The kernel driver owns USB enumeration, the streaming engine, the ALSA PCM
 * device and the raw device transports.  Policy and device models (matrix,
 * faders, EQ, meters, profiles, scenes) live in userspace.  The driver
 * reports hardware facts and capabilities; it does not name a device model.
 *
 * This header is the single source of truth for the ABI.  The Rust side
 * lives in ui/totalcontrol/totalcontrol-core/src/fireface/abi.rs and must be
 * updated together with it.
 */
#ifndef _FF_UAPI_H
#define _FF_UAPI_H

#include <linux/types.h>
#include <linux/ioctl.h>

#define FF_ABI_VERSION	9

/* Largest payload in one FF_HWDEP_IOCTL_VENDOR request. */
#define FF_HWDEP_MAX_DATA	64
/* Largest register burst accepted by FF_HWDEP_IOCTL_WRITE_WORDS. */
#define FF_HWDEP_MAX_WORDS	63
/* Largest payload in one event record (state, meter or status). */
#define FF_EVENT_MAX		704

/* Meter event flags: the heartbeat is the first after a session whose
 * page phase could not be anchored. */
#define FF_EV_FLAG_PHASE_LOST	1u
/* Payload is one complete heartbeat (RMS half, then peak half). */
#define FF_EV_FLAG_METER_PAIR	2u

enum ff_transport {
	FF_XPORT_INTERRUPT	= 0,	/* interrupt audio transport */
	FF_XPORT_ISO		= 1,	/* ISO audio transport */
};

/* ff_hwdep_info.caps: control transports and host-visible roles. */
#define FF_CAP_VENDOR		(1u << 0)	/* vendor control transfers */
#define FF_CAP_WORDS		(1u << 1)	/* register word transport */
#define FF_CAP_METERS		(1u << 2)	/* meter events */
#define FF_CAP_STATUS		(1u << 3)	/* status endpoint events */
#define FF_CAP_PCM		(1u << 4)	/* ALSA PCM device */
#define FF_CAP_SNAPSHOT		(1u << 5)	/* live snapshot record */
#define FF_CAP_DIAGNOSTICS	(1u << 6)	/* diagnostics record */

enum ff_event_type {
	FF_EV_METER		= 1,	/* raw meter packet */
	FF_EV_STATE		= 2,	/* raw register words (LE u32) */
	FF_EV_STATUS		= 3,	/* raw status endpoint packet */
	FF_EV_LATCH		= 4,	/* output-latch transition */
};

struct ff_hwdep_info {
	__u32 abi;
	__u32 caps;		/* FF_CAP_* */
	__u32 transport;	/* enum ff_transport */
	__u32 channels;		/* maximum channels each way at 1x */
	__u32 inputs;		/* hardware input channels */
	__u32 outputs;		/* hardware output channels */
	__u32 playback;		/* software playback channels */
	__u32 matrix_sources;	/* inputs + playback */
	__u32 meter_bytes;	/* 0 = no meter endpoint */
	__u32 state_ep;
	__u32 meter_ep;
	__u32 status_ep;
	__u32 rate;		/* current rate in Hz */
	__u32 firmware;		/* vendor firmware revision, 0 if unknown */
	__u32 card;		/* ALSA card number */
	__u32 pcm_device;	/* ALSA PCM device number */
	__u32 vendor;		/* USB idVendor */
	__u32 product;		/* USB idProduct */
	__u32 bcd_device;	/* USB bcdDevice */
	char serial[32];	/* USB serial string, may be empty */
	__u32 reserved[16];
};

/*
 * One vendor request.  direction 0 sends `length` bytes from `data`
 * (usually zero); direction 1 reads into `data` and updates `length` with
 * the number of bytes received.
 */
struct ff_hwdep_vendor {
	__u8 request;
	__u8 direction;
	__u16 value;
	__u16 index;
	__u16 length;
	__u16 flags;
	__u8 data[FF_HWDEP_MAX_DATA];
};

/* One register word (address + value); the kernel adds odd parity. */
struct ff_hwdep_word {
	__u16 addr;
	__u16 value;
};

struct ff_hwdep_words {
	__u32 count;
	__u32 flags;
	struct ff_hwdep_word words[FF_HWDEP_MAX_WORDS];
};

/* Header of every record returned by read(2). */
struct ff_hwdep_event_hdr {
	__u32 type;		/* enum ff_event_type */
	__u32 size;		/* payload bytes after this header */
	__u32 seq;		/* monotonically increasing per event */
	__u32 flags;		/* event-type specific */
};

/* Diagnostics: accumulated history and watermarks.  Counters are either
 * cumulative for the driver instance or per-run; run_seq increments on every
 * engine start and scopes the per-run values.  urb_errors is a consecutive
 * error gauge, not a total. */
struct ff_hwdep_diagnostics {
	__u32 out_urbs;
	__u32 out_gaps;
	__u32 out_shorts;
	__u32 out_status_errors;
	__u32 urb_errors;
	__u32 recoveries;
	__u32 events_dropped;
	__u32 meter_urbs;
	__u32 state_urbs;
	__u32 status_urbs;
	__u32 in_urbs;
	__u32 in_gaps;
	__u32 in_shorts;
	__u32 in_status_errors;
	__u32 out_underruns;
	__u32 in_overruns;
	__u32 out_capacity_err;
	__u32 out_pkt_len_err;
	__u32 out_pkt_status;
	__u32 in_status_err;
	__u32 in_short;
	__u32 in_misaligned;
	__u32 in_bad_length;
	__u32 out_bad_length;
	__u32 in_pos_gap;
	__u32 in_frames_mismatch;
	__u32 rate_mismatch;
	__u32 out_fb_ahead;
	__u32 out_fb_stale;
	__u32 hw_ptr_mismatch;
	__u32 delta_zero;
	__u32 run_seq;
	/* Live scheduling watermarks and derived counters. */
	__u32 ring_min_filled;
	__u32 ring_missed;
	__u32 ring_starved;
	__u64 ring_max_stall_ns;
	__u32 gate_deferred;
	__u32 gate_overdue;
	__u32 gate_max_pending;
	__s32 out_lag_min;
	__s32 out_lag_max;
	__u32 out_prefills;
	__u32 out_prefill_timeouts;
	__u32 meter_paused;
	__u32 meter_pairs;
	__u32 meter_realigns;
	__u64 in_total;
	__u32 pitch_clamps;
	__u32 rate_skips;
	__u32 rate_writes;
	__u32 reserved[5];
};

/* Largest rate table in one FF_HWDEP_IOCTL_ENVELOPE reply. */
#define FF_HWDEP_MAX_RATES	16

/* One supported rate and the ALSA geometry the driver accepts at it.
 * period_floor is one URB: the smallest period, and (with disable-tsched)
 * the smallest graph quantum. buffer_min is the smallest ring. Both are
 * computed by the same functions that refine hw_params, so the driver is
 * the single owner of the achievable envelope. */
struct ff_hwdep_rate_env {
	__u32 rate;		/* Hz */
	__u32 period_floor;	/* frames */
	__u32 buffer_min;	/* frames */
	__u32 capture_channels;	/* active capture channels at this rate */
	__u32 playback_channels;/* active playback channels at this rate */
};

struct ff_hwdep_envelope {
	__u32 count;		/* valid entries in rates[] */
	__u32 urb_frames;	/* base URB frames */
	__u32 nurbs;		/* URBs in flight per endpoint */
	__u32 iso;		/* 1 ISO transport, 0 interrupt */
	__u32 iso_packets;	/* ISO packets per URB */
	__u32 fb_delay;		/* feedback delay in USB packets */
	__u32 out_lead;		/* output lead in frames */
	__u32 lead_bias;	/* reference lead bias in frames */
	__u32 reserved[4];
	struct ff_hwdep_rate_env rates[FF_HWDEP_MAX_RATES];
};

/* One direction of the live stream snapshot. */
struct ff_hwdep_stream {
	__u32 present;		/* stream prepared/running */
	__u32 state;		/* ALSA state after prepare/start */
	__u64 appl_ptr;		/* application position */
	__u64 hw_ptr;		/* driver position */
	__u32 fill;		/* frames queued in the ring */
	__u32 inflight;		/* frames in submitted USB URBs */
	__u32 delay;		/* snd_pcm_delay equivalent */
	__u32 period;		/* period_size */
	__u32 buffer;		/* buffer_size */
	__u32 urb_frames;	/* frames per URB at the current rate */
	__u32 nurbs;
	__u32 iso_packets;
	__u64 last_complete_ns;	/* host time of the last completion */
	__u64 device_pos;	/* device position at the last completion */
	__u32 xruns;		/* per-run underrun/overrun count */
	__u32 flags;
	__u32 reserved[8];
};

/* Live, atomic snapshot: queue state for both directions and the device
 * clock in one sample.  The caller sets `size`; the driver fills up to that
 * size and reports the full struct size.  New fields are appended, so an
 * older host reads a shorter prefix and ignores the rest. */
struct ff_hwdep_snapshot {
	__u32 abi;
	__u32 size;		/* in: caller size; out: struct size */
	__u32 run_seq;
	__u32 rate;
	__u64 timestamp_ns;	/* device clock sample: host time */
	__u64 timestamp_pos;	/* device position at that host time */
	__u32 timestamp_phase;
	__u32 rate_meas;
	__u64 anchor_ns;	/* SOF anchor: sof_ns(f) = anchor_ns + f*125000 */
	__u64 frames;		/* anchor frame index */
	__u32 wrap;		/* device position wrap */
	__u32 flags;
	struct ff_hwdep_stream out;
	struct ff_hwdep_stream in;
	__u32 reserved[16];
};

/* Set the device clock rate while the engine is idle (no PCM owner).  The
 * driver adopts the rate, commits it to the device and confirms it from the
 * state-word rate readback, then bumps run_seq so the host reapplies the
 * scene.  Idle-only: -EBUSY while an owner holds the PCM, -EINVAL for an
 * unsupported rate, -EIO if the readback does not confirm. */
struct ff_hwdep_set_rate {
	__u32 rate;		/* in: requested rate, Hz */
	__u32 flags;		/* in: must be 0 */
	__u32 confirmed;	/* out: state-word rate after the write, Hz */
	__u32 reserved[5];
};

#define FF_HWDEP_IOCTL_INFO \
	_IOR('F', 0x10, struct ff_hwdep_info)
#define FF_HWDEP_IOCTL_VENDOR \
	_IOWR('F', 0x11, struct ff_hwdep_vendor)
#define FF_HWDEP_IOCTL_WRITE_WORDS \
	_IOW('F', 0x12, struct ff_hwdep_words)
#define FF_HWDEP_IOCTL_DIAGNOSTICS \
	_IOR('F', 0x13, struct ff_hwdep_diagnostics)
#define FF_HWDEP_IOCTL_ENVELOPE \
	_IOR('F', 0x14, struct ff_hwdep_envelope)
#define FF_HWDEP_IOCTL_SET_RATE \
	_IOWR('F', 0x16, struct ff_hwdep_set_rate)
#define FF_HWDEP_IOCTL_SNAPSHOT \
	_IOWR('F', 0x17, struct ff_hwdep_snapshot)

#endif /* _FF_UAPI_H */
