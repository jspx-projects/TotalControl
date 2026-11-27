/* SPDX-License-Identifier: GPL-2.0-only */
#ifndef FF_GATE_H
#define FF_GATE_H

#ifdef __KERNEL__
#include <linux/types.h>
#else
#include <stdbool.h>
#endif

/*
 * The duplex period gate aligns the two directions only when they have the
 * same ALSA period: with equal periods both reach a common boundary. Unequal
 * periods have no common boundary, so a withheld wake would be deferred on
 * every cycle. Such a pair is signalled independently at its own boundary.
 */
static inline bool ff_gate_together(unsigned int p_period,
				      unsigned int c_period)
{
	return p_period > 0 && p_period == c_period;
}

#endif /* FF_GATE_H */
