// SPDX-License-Identifier: GPL-2.0-only
/*
 * Host test for the ISO cadence convergence (ffuc-cadence.h).
 *
 *   make -C kernel test
 *
 * The stop verdict must judge a steady rate — right or wrong — and must
 * NOT judge a run stopped inside the startup ramp or a short
 * renegotiation fragment.  Packet sequences are synthetic; the add/drop
 * pattern is generated with the device's fractional accumulator.
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "../ffuc-cadence.h"

static int failures;

#define CHECK(cond, ...) do {						\
	if (!(cond)) {							\
		failures++;						\
		fprintf(stderr, "FAIL %s:%d: ", __func__, __LINE__);	\
		fprintf(stderr, __VA_ARGS__);				\
		fputc('\n', stderr);					\
	}								\
} while (0)

/* Generate the device's per-packet frame counts at a steady rate: the
 * fractional accumulator carries rate/8000, the add/drop pattern. */
static void gen_rate(unsigned int rate, unsigned int n, unsigned int *out)
{
	unsigned int acc = 0, i;

	for (i = 0; i < n; i++) {
		acc += rate;
		out[i] = acc / 8000;
		acc %= 8000;
	}
}

/* Feed n packets as URBs of 8, the shipped geometry. */
static void feed(struct ffuc_cadence *c, unsigned int rate,
		 const unsigned int *frames, unsigned int n)
{
	unsigned int i;

	for (i = 0; i < n; i += 8) {
		unsigned int k, sum = 0, cnt = n - i < 8 ? n - i : 8;

		for (k = 0; k < cnt; k++)
			sum += frames[i + k];
		ffuc_cadence_window(c, rate, sum, cnt);
	}
}

static void feed_steady(struct ffuc_cadence *c, unsigned int rate,
			unsigned int pkts)
{
	unsigned int frames[512];

	while (pkts) {
		unsigned int n = pkts < 512 ? pkts : 512;

		gen_rate(rate, n, frames);
		feed(c, rate, frames, n);
		pkts -= n;
	}
}

/* A steady correct rate converges to itself and raises no verdict. */
static void test_steady_correct(void)
{
	struct ffuc_cadence c;
	unsigned int rates[] = { 44100, 48000, 96000, 192000 };
	unsigned int r;

	for (r = 0; r < 4; r++) {
		ffuc_cadence_reset(&c);
		feed_steady(&c, rates[r], 8 * 40);
		CHECK(c.stable_mean == rates[r],
		      "rate %u: stable %u", rates[r], c.stable_mean);
		CHECK(!ffuc_cadence_stop_mismatch(&c, rates[r]),
		      "rate %u: false verdict", rates[r]);
	}
}

/* The startup ramp (fewer frames, a header-only packet) must not yield a
 * verdict while it is moving, and must converge once steady. */
static void test_ramp_then_steady(void)
{
	struct ffuc_cadence c;
	unsigned int ramp[16] = { 0, 3, 4, 5, 5, 6, 6, 6,
				  6, 6, 6, 6, 6, 6, 6, 6 };

	ffuc_cadence_reset(&c);
	feed(&c, 48000, ramp, 16);
	CHECK(!ffuc_cadence_stop_mismatch(&c, 48000),
	      "ramp produced a verdict (stable %u)", c.stable_mean);
	feed_steady(&c, 48000, 8 * 40);
	CHECK(c.stable_mean == 48000, "post-ramp stable %u", c.stable_mean);
	CHECK(!ffuc_cadence_stop_mismatch(&c, 48000), "post-ramp verdict");
}

/* A short renegotiation fragment — the device-observed false positive:
 * 7 packets carrying 36 frames at 48 kHz — has no converged cadence. */
static void test_short_fragment(void)
{
	struct ffuc_cadence c;
	unsigned int frag[8] = { 0, 5, 5, 6, 6, 6, 6, 2 };

	ffuc_cadence_reset(&c);
	feed(&c, 48000, frag, 7);
	CHECK(c.stable_mean == 0, "fragment converged (%u)", c.stable_mean);
	CHECK(!ffuc_cadence_stop_mismatch(&c, 48000), "fragment verdict");
	/* Even a few windows of ramp-shaped data must not converge. */
	feed(&c, 48000, frag, 8);
	feed(&c, 48000, frag, 8);
	CHECK(!ffuc_cadence_stop_mismatch(&c, 48000),
	      "repeated fragment verdict (stable %u)", c.stable_mean);
}

/* A genuine wrong rate IS flagged: the device streams 96 kHz while the
 * committed rate is 48 kHz. */
static void test_wrong_rate_flagged(void)
{
	struct ffuc_cadence c;

	ffuc_cadence_reset(&c);
	feed_steady(&c, 96000, 8 * 40);	/* device at 96k */
	CHECK(c.stable_mean == 96000, "wrong-rate stable %u", c.stable_mean);
	CHECK(ffuc_cadence_stop_mismatch(&c, 48000),
	      "wrong rate not flagged (stable %u vs 48000)", c.stable_mean);
	/* Adjacent supported rates are still separated by the ±1% band. */
	CHECK(ffuc_cadence_stop_mismatch(&c, 44100), "44.1 vs 96 not flagged");
}

/* A wrong rate that persists only briefly, then stops, is the failure
 * the fixed minimum missed: convergence still flags it once steady. */
static void test_short_wrong_run(void)
{
	struct ffuc_cadence c;

	ffuc_cadence_reset(&c);
	feed_steady(&c, 96000, 8 * 30);	/* ~30 ms at the wrong rate */
	CHECK(ffuc_cadence_stop_mismatch(&c, 48000),
	      "short wrong run not flagged (stable %u)", c.stable_mean);
}

/* A mid-run rate change clears convergence and re-establishes it. */
static void test_rate_change(void)
{
	struct ffuc_cadence c;

	ffuc_cadence_reset(&c);
	feed_steady(&c, 48000, 8 * 40);
	CHECK(c.stable_mean == 48000, "pre-change stable %u", c.stable_mean);
	feed_steady(&c, 96000, 8 * 40);
	CHECK(c.stable_mean == 96000, "post-change stable %u", c.stable_mean);
	CHECK(ffuc_cadence_stop_mismatch(&c, 48000), "stale verdict after change");
	CHECK(!ffuc_cadence_stop_mismatch(&c, 96000), "false verdict at new rate");
}

/* A normal rate change must not count as a mismatch: the engine run
 * resets convergence, the cadence settles through a moving mean that
 * renders no verdict against either rate, and only the new steady rate
 * converges. This is the periodic-guard verdict the fix relies on. */
static void test_rate_settling_no_verdict(void)
{
	struct ffuc_cadence c;
	unsigned int ramp[16] = { 6, 6, 6, 6, 7, 7, 8, 8,
				  9, 9, 10, 10, 11, 11, 12, 12 };

	ffuc_cadence_reset(&c);
	feed_steady(&c, 48000, 8 * 40);
	CHECK(c.stable_mean == 48000, "pre-settle stable %u", c.stable_mean);
	/* A rate change starts a fresh engine run: convergence resets. */
	ffuc_cadence_reset(&c);
	feed(&c, 96000, ramp, 16);	/* settling ramp 48k -> 96k */
	CHECK(!ffuc_cadence_stop_mismatch(&c, 96000),
	      "settling counted vs new rate (stable %u)", c.stable_mean);
	CHECK(!ffuc_cadence_stop_mismatch(&c, 48000),
	      "settling counted vs old rate (stable %u)", c.stable_mean);
	feed_steady(&c, 96000, 8 * 40);
	CHECK(c.stable_mean == 96000, "post-settle stable %u", c.stable_mean);
	CHECK(!ffuc_cadence_stop_mismatch(&c, 96000), "post-settle verdict");
}

/* Convergence needs three windows, not one: a single window is unknown. */
static void test_needs_three_windows(void)
{
	struct ffuc_cadence c;

	ffuc_cadence_reset(&c);
	feed_steady(&c, 48000, 80);	/* one window */
	CHECK(c.stable_mean == 0, "one window converged (%u)", c.stable_mean);
	feed_steady(&c, 48000, 80);	/* two windows: one agreement */
	CHECK(c.stable_mean == 0, "two windows converged (%u)", c.stable_mean);
	feed_steady(&c, 48000, 80);	/* three windows: two agreements */
	CHECK(c.stable_mean == 48000, "three windows stable %u", c.stable_mean);
}

/* The DDS decode recovers the programmed clock within 1 Hz across every
 * rate and the full ±5% varispeed range, and the judge does not flag a
 * legitimate pitch as a mismatch (the rounding stays far inside ±1%). */
static void test_dds_clock_decode(void)
{
	const unsigned int rates[] = {
		32000, 44100, 48000, 64000, 88200, 96000, 128000, 176400, 192000
	};
	const double pitches[] = { -5.0, -3.0, -1.0, 0.0, 1.0, 3.0, 5.0 };
	unsigned int r, p;

	for (r = 0; r < sizeof(rates) / sizeof(rates[0]); r++) {
		unsigned int rate = rates[r];
		unsigned int band = rate > 114000 ? 4 : rate > 57000 ? 2 : 1;
		unsigned int base = rate / band;

		for (p = 0; p < sizeof(pitches) / sizeof(pitches[0]); p++) {
			double scale = 1.0 + pitches[p] / 100.0;
			u64 denom = (u64)(base * scale + 0.5);
			u64 p1 = (u64)(0x5f5e10000000ULL / (double)denom + 0.5);
			u32 val = (u32)(p1 >> 16);
			u32 idx = (u32)(p1 & 0xff00);
			u32 decoded = ffuc_dds_clock(val, idx, rate);
			u32 measured = (u32)(rate * scale + 0.5);
			int diff = (int)decoded - (int)measured;

			if (diff < 0)
				diff = -diff;
			CHECK(diff <= 1, "rate %u pitch %.1f%%: decoded %u vs %u",
			      rate, pitches[p], decoded, measured);
			CHECK(!ffuc_cadence_mismatch(decoded, measured),
			      "rate %u pitch %.1f%%: false mismatch (clock %u meas %u)",
			      rate, pitches[p], decoded, measured);
		}
	}
}

int main(void)
{
	test_steady_correct();
	test_ramp_then_steady();
	test_short_fragment();
	test_wrong_rate_flagged();
	test_short_wrong_run();
	test_rate_change();
	test_rate_settling_no_verdict();
	test_needs_three_windows();
	test_dds_clock_decode();
	if (failures) {
		fprintf(stderr, "cadence-test: %d failure(s)\n", failures);
		return 1;
	}
	printf("cadence-test: ok\n");
	return 0;
}
