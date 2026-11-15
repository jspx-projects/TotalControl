// SPDX-License-Identifier: GPL-2.0-only
/*
 * Host test for the UC meter heartbeat pairing (ffuc-meter-pair.h).
 *
 *   make -C kernel test
 *
 * Pages are synthetic.  A peak page ('P') carries eight non-zero AN
 * records at bytes 108..139.  An RMS page ('R') carries zeros there, as it
 * does while the digital inputs are silent.  A 'B' page is an RMS page
 * with all eight words non-zero, which the shape cannot tell from a peak
 * page.  Byte 0 tags the role and byte 1 numbers the page.
 */
#include <stdio.h>
#include <stdlib.h>

#include "../ffuc-meter-pair.h"

#define LEN 324

static int failures;

#define CHECK(cond, ...) do {						\
	if (!(cond)) {							\
		failures++;						\
		fprintf(stderr, "FAIL %s:%d: ", __func__, __LINE__);	\
		fprintf(stderr, __VA_ARGS__);				\
		fputc('\n', stderr);					\
	}								\
} while (0)

static void page(u8 *p, char role, unsigned int n)
{
	unsigned int i;

	memset(p, 0, LEN);
	p[0] = (u8)role;
	p[1] = (u8)n;
	if (role == 'P' || role == 'B')
		for (i = 0; i < 8; i++) {
			p[108 + i * 4 + 1] = 0xff;	/* AN record floor */
			p[108 + i * 4 + 2] = 0x01;
		}
}

struct run {
	u8 pair[2 * LEN];
	unsigned int held;
	bool lost;
	char out[64][3];	/* emitted pairs: roles of both halves + 'u' */
	unsigned int pairs;
	unsigned int realigns;
};

/* Feed @roles ('R' RMS, 'P' peak, 'B' RMS of peak shape). */
static void feed(struct run *r, const char *roles)
{
	static unsigned int n;
	u8 p[LEN];

	for (; *roles; roles++) {
		page(p, *roles, n++);
		switch (ffuc_meter_feed(r->pair, &r->held, &r->lost, p, LEN)) {
		case FFUC_METER_PAIR:
		case FFUC_METER_PAIR_UNVERIFIED:
			r->out[r->pairs][0] = (char)r->pair[0];
			r->out[r->pairs][1] = (char)r->pair[LEN];
			r->out[r->pairs][2] = r->lost ? 'u' : '-';
			r->pairs++;
			break;
		case FFUC_METER_REALIGNED:
			r->realigns++;
			break;
		case FFUC_METER_HELD:
			break;
		}
	}
}

/* Pairs from index @from on are all RMS-then-peak. */
static bool ordered_from(const struct run *r, unsigned int from)
{
	unsigned int i;

	for (i = from; i < r->pairs; i++)
		if (r->out[i][0] != 'R' || r->out[i][1] != 'P')
			return false;
	return true;
}

static void steady_session_keeps_every_heartbeat(void)
{
	struct run r = { .lost = false };

	feed(&r, "RPRPRPRPRP");
	CHECK(r.pairs == 5 && r.realigns == 0, "pairs %u realigns %u",
	      r.pairs, r.realigns);
	CHECK(ordered_from(&r, 0), "steady pairs out of order");
}

/* A session whose phase is unknown and whose first page is the peak half:
 * every heartbeat after the first is RMS-then-peak, not only the first. */
static void peak_first_session_is_realigned_for_good(void)
{
	struct run r = { .lost = true };

	feed(&r, "PRPRPRPRPRPR");
	CHECK(r.realigns == 1, "realigns %u", r.realigns);
	CHECK(r.pairs == 5, "pairs %u", r.pairs);
	CHECK(ordered_from(&r, 0), "pairs after the first are peak-first");
	CHECK(!r.lost, "phase still unverified after a decidable pair");
}

/* The order slips inside a verified session: the RMS half arrives twice
 * (captured at streaming engine starts and stops).  The pair of two RMS
 * halves is not emitted, so no RMS page is ever read as a peak page, and
 * every later pair is ordered. */
static void slip_inside_a_session_is_realigned(void)
{
	struct run r = { .lost = false };

	feed(&r, "RPRPRP" "RR" "PRPRPRPRPR");
	CHECK(r.realigns == 1, "realigns %u", r.realigns);
	CHECK(ordered_from(&r, 0), "a pair without a peak half was emitted, "
	      "or a later pair is peak-first");
	CHECK(r.pairs == 8, "pairs %u", r.pairs);
	CHECK(!r.lost, "phase still unverified after the next decidable pair");
}

/* A peak half lost on the way (the RMS halves of two heartbeats arrive
 * back to back) is the same slip. */
static void lost_peak_half_is_realigned(void)
{
	struct run r = { .lost = false };

	feed(&r, "RPRPR" "RPRPRPRP");
	CHECK(ordered_from(&r, 0), "pairs around the lost half are out of order");
	CHECK(r.realigns == 1 && r.pairs == 6, "realigns %u pairs %u",
	      r.realigns, r.pairs);
}

/* If peak halves stopped carrying the shape, pairs without a peak shape
 * would follow each other.  Only the first is dropped; the rest pass
 * flagged, so the meters never stop. */
static void pairs_without_a_peak_shape_never_stop_the_meters(void)
{
	struct run r = { .lost = false };

	feed(&r, "RP" "RRRRRRRR");
	CHECK(r.realigns == 1, "realigns %u", r.realigns);
	CHECK(r.pairs == 4, "pairs %u", r.pairs);
	CHECK(r.out[1][2] == 'u' && r.out[2][2] == 'u' && r.out[3][2] == 'u',
	      "pairs after the drop are not flagged unverified");
}

/* A pair the shape cannot decide passes in arrival order.  It is flagged
 * while the phase is unknown and unflagged once a pair has decided it. */
static void undecidable_pairs_keep_arrival_order(void)
{
	struct run r = { .lost = true };

	feed(&r, "BPBP");
	CHECK(r.pairs == 2 && r.realigns == 0, "pairs %u realigns %u",
	      r.pairs, r.realigns);
	CHECK(r.out[0][2] == 'u' && r.out[1][2] == 'u',
	      "undecided pairs not flagged");
	feed(&r, "RP" "BP");
	CHECK(r.out[2][2] == '-' && r.out[3][2] == '-',
	      "pairs flagged after a decidable pair");
}

int main(void)
{
	steady_session_keeps_every_heartbeat();
	peak_first_session_is_realigned_for_good();
	slip_inside_a_session_is_realigned();
	lost_peak_half_is_realigned();
	pairs_without_a_peak_shape_never_stop_the_meters();
	undecidable_pairs_keep_arrival_order();
	if (failures) {
		fprintf(stderr, "meter-pair-test: %d failure(s)\n", failures);
		return EXIT_FAILURE;
	}
	printf("meter-pair-test: ok\n");
	return EXIT_SUCCESS;
}
