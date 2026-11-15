/* SPDX-License-Identifier: GPL-2.0-only */
/*
 * Fireface UC meter heartbeat pairing.
 *
 * The UC sends its 0x85 meter pages as alternating RMS and peak halves; the
 * driver emits them as RMS-then-peak pairs.  This file holds the pairing
 * decision only, free of USB and ALSA, so kernel/test/ can drive it with
 * page sequences on the host.
 */
#ifndef FFUC_METER_PAIR_H
#define FFUC_METER_PAIR_H

#ifdef __KERNEL__
#include <linux/string.h>
#include <linux/types.h>
#else
#include <stdbool.h>
#include <stdint.h>
#include <string.h>
typedef uint8_t u8;
typedef uint32_t u32;
#endif

/* Number of AN input record words that are not zero.  The peak half
 * publishes the eight analog records (idle floors included); the RMS half
 * carries input-bank slot words in that address range, which are zero
 * while the digital inputs are silent. */
static inline unsigned int ffuc_an_record_words(const u8 *page,
						unsigned int len)
{
	unsigned int i, n = 0;

	if (len < 108 + 8 * 4)
		return 0;
	for (i = 0; i < 8; i++) {
		u32 record;

		memcpy(&record, page + 108 + i * 4, sizeof(record));
		if (record)
			n++;
	}
	return n;
}

enum ffuc_meter_feed {
	FFUC_METER_HELD,		/* first half stored, no pair yet */
	FFUC_METER_PAIR,		/* pair[] is RMS then peak */
	FFUC_METER_PAIR_UNVERIFIED,	/* arrival order, roles unverified */
	FFUC_METER_REALIGNED,		/* peak half dropped, no pair */
};

/*
 * Feed one page of @len bytes.  @pair holds 2 * @len bytes, @held is 0 or
 * @len (bytes of the first half stored), @lost is the session's
 * phase-unknown state.
 *
 * Every completed pair is checked by the record-table shape.  The page
 * order can slip by one inside a running session, so a pair that is
 * decidably peak-first is not emitted: its peak half is dropped and its
 * RMS half opens the next pair, which restores the order for every
 * following pair.
 *
 * A slip arrives as two RMS halves back to back.  Once a pair has
 * decided the order, a pair in which neither half has the peak shape has
 * no peak half, so it is not emitted either: its first half is dropped,
 * its second opens the next pair, and the phase counts as unknown again
 * until a pair decides it.  While the phase is unknown such a pair is
 * emitted, so the meters keep running even if peak halves stop carrying
 * the shape.
 *
 * Any other pair the shape cannot decide is emitted in arrival order; it
 * is reported unverified while @lost is set.
 */
static inline enum ffuc_meter_feed
ffuc_meter_feed(u8 *pair, unsigned int *held, bool *lost, const u8 *page,
		unsigned int len)
{
	unsigned int a, b;

	memcpy(pair + *held, page, len);
	if (!*held) {
		*held = len;
		return FFUC_METER_HELD;
	}
	*held = 0;
	a = ffuc_an_record_words(pair, len);
	b = ffuc_an_record_words(pair + len, len);
	if (a >= b + 4) {
		memcpy(pair, pair + len, len);
		*held = len;
		*lost = false;
		return FFUC_METER_REALIGNED;
	}
	if (b >= a + 4) {
		*lost = false;
		return FFUC_METER_PAIR;
	}
	if (a < 4 && b < 4 && !*lost) {
		memcpy(pair, pair + len, len);
		*held = len;
		*lost = true;
		return FFUC_METER_REALIGNED;
	}
	return *lost ? FFUC_METER_PAIR_UNVERIFIED : FFUC_METER_PAIR;
}

#endif
