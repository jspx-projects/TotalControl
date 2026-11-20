/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Fireface ISO cadence convergence.
 *
 * The stop-path rate verdict judges the mean frames/packet only once
 * successive windows agree: the startup ramp is a moving mean and fails
 * the agreement test, while a steady rate — right or wrong — converges.
 * A run stopped before the mean settles has no cadence to judge, so the
 * verdict is skipped dynamically rather than by a fixed sample count.
 * This file holds the convergence decision only, free of USB and ALSA,
 * so kernel/test/ can drive it with packet sequences on the host.
 */
#ifndef FFUC_CADENCE_H
#define FFUC_CADENCE_H

#ifdef __KERNEL__
#include <linux/types.h>
#else
#include <stdbool.h>
#include <stdint.h>
typedef uint32_t u32;
typedef uint64_t u64;
#endif

/* Cadence convergence window, in packets: one add/drop period of the
 * lowest fractional rate (44.1 kHz = 441/80 frames per packet), so a
 * window mean averages the clock's add/drop exactly.  Integer-frame
 * rates converge in any window.  The verdict needs successive windows
 * to agree, not a fixed packet count. */
#define FFUC_CADENCE_WIN_PKTS	80

struct ffuc_cadence {
	u64 win_frames;		/* frames summed in the current window */
	u32 win_pkts;		/* packets in the current window */
	u32 prev_mean;		/* last completed window mean, Hz (0 = none) */
	u32 stable_mean;	/* converged mean, Hz (0 = not converged) */
	u32 agree;		/* consecutive window agreements */
};

static inline void ffuc_cadence_reset(struct ffuc_cadence *c)
{
	c->win_frames = 0;
	c->win_pkts = 0;
	c->prev_mean = 0;
	c->stable_mean = 0;
	c->agree = 0;
}

/* The ±1% band: a mean outside it disagrees with the committed rate. */
static inline bool ffuc_cadence_mismatch(u32 rate, u32 mean)
{
	return rate && (mean > rate + rate / 100 || mean + rate / 100 < rate);
}

/* Accumulate one capture URB's frames and track stationarity of the
 * window mean.  Two consecutive agreements (three windows) hold the mean
 * stationary before it is trusted; a disagreement clears it.  Called per
 * capture URB under fb_lock. */
static inline void ffuc_cadence_window(struct ffuc_cadence *c, u32 rate,
				       unsigned int urb_frames, unsigned int pkts)
{
	u32 mean, tol;

	c->win_frames += urb_frames;
	c->win_pkts += pkts;
	if (c->win_pkts < FFUC_CADENCE_WIN_PKTS)
		return;
	mean = (u32)(c->win_frames * 8000 / c->win_pkts);
	if (c->prev_mean) {
		tol = rate / 200;
		if (tol < 1)
			tol = 1;
		if (mean > c->prev_mean ? mean - c->prev_mean <= tol :
					  c->prev_mean - mean <= tol) {
			if (++c->agree >= 2)
				c->stable_mean = mean;
		} else {
			c->agree = 0;
			c->stable_mean = 0;
		}
	}
	c->prev_mean = mean;
	c->win_frames = 0;
	c->win_pkts = 0;
}

/* The stop verdict: true when a converged mean disagrees with the rate.
 * False when no mean has converged (run too short, or still ramping). */
static inline bool ffuc_cadence_stop_mismatch(const struct ffuc_cadence *c,
					      u32 rate)
{
	return c->stable_mean && ffuc_cadence_mismatch(rate, c->stable_mean);
}

#endif /* FFUC_CADENCE_H */
