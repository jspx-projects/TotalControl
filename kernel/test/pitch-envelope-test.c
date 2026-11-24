// SPDX-License-Identifier: GPL-2.0-only
/*
 * Pitch envelope model tests (host, no kernel).
 *
 * The ISO transport carries at most max_frames frames per microframe
 * packet; the programmed clock (nominal * (1 + pitch/100)) must stay at or
 * below 8000 * max_frames. These tests exercise the pure decode/clamp law
 * in ffuc-cadence.h exhaustively: every supported rate across the full
 * pitch range, the clamp invariants, and the desk-verified boundary.
 *
 * Build: cc -std=c11 -Wall -Wextra -Werror -o pitch-envelope-test \
 *            pitch-envelope-test.c
 */
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

#include "../ffuc-cadence.h"

static int failures;

#define CHECK(cond, ...) do { \
	if (!(cond)) { \
		fprintf(stderr, "FAIL %s:%d: ", __func__, __LINE__); \
		fprintf(stderr, __VA_ARGS__); \
		fprintf(stderr, "\n"); \
		failures++; \
	} \
} while (0)

/* The nine supported rates and their band's max_frames (the mode table:
 * 1x <= 48k -> 7, 2x <= 96k -> 13, 4x -> 25). */
static const struct { unsigned int rate; unsigned int max_frames; } rates[] = {
	{ 32000, 7 }, { 44100, 7 }, { 48000, 7 },
	{ 64000, 13 }, { 88200, 13 }, { 96000, 13 },
	{ 128000, 25 }, { 176400, 25 }, { 192000, 25 },
};

static unsigned int family_base(unsigned int rate)
{
	switch (rate) {
	case 32000: case 64000: case 128000: return 32000;
	case 44100: case 88200: case 176400: return 44100;
	default: return 48000;
	}
}

static unsigned int family_idx(unsigned int rate)
{
	switch (rate) {
	case 32000: case 64000: case 128000: return 0;
	case 44100: case 88200: case 176400: return 1;
	default: return 2;
	}
}

/* Build the DDS word pair exactly as the host pitch write does:
 * DDS = K / (base * (1 + pitch/100)), val = DDS >> 16,
 * idx = (DDS & 0xff00) | family. */
static void dds_word(unsigned int rate, double pitch, u32 *val, u32 *idx)
{
	double base = family_base(rate);
	u64 dds = (u64)llround(0x5f5e10000000ULL / (base * (1.0 + pitch / 100.0)));

	*val = (u32)(dds >> 16);
	*idx = (u32)((dds & 0xff00) | family_idx(rate));
}

/* The decode must reproduce the programmed clock within the word-grid
 * truncation (the low 8 bits of the divisor are not written). */
static void test_decode_matches_programmed(void)
{
	for (size_t r = 0; r < sizeof(rates) / sizeof(rates[0]); r++) {
		unsigned int rate = rates[r].rate;

		for (int p = -500; p <= 500; p++) {
			double pitch = p / 100.0;
			u32 val, idx;
			double want = rate * (1.0 + pitch / 100.0);
			u32 got;

			dds_word(rate, pitch, &val, &idx);
			got = ffuc_dds_clock(val, idx, rate);
			CHECK(fabs((double)got - want) < 2.0,
			      "rate %u pitch %.2f: decoded %u vs programmed %.1f",
			      rate, pitch, got, want);
		}
	}
}

/* Exhaustive clamp sweep: in-envelope words pass untouched; over-capacity
 * words clamp to the tightest grid point at or below the ceiling, preserve
 * the family, and are idempotent. */
static void test_clamp_sweep(void)
{
	for (size_t r = 0; r < sizeof(rates) / sizeof(rates[0]); r++) {
		unsigned int rate = rates[r].rate;
		unsigned int mf = rates[r].max_frames;
		u64 cap = (u64)mf * 8000;
		u32 prev_clock = 0;

		for (int p = -500; p <= 500; p++) {
			double pitch = p / 100.0;
			u32 val, idx, v2, i2;
			u32 clock, clamped_clock;
			bool over, changed;

			dds_word(rate, pitch, &val, &idx);
			clock = ffuc_dds_clock(val, idx, rate);
			over = (u64)clock > cap;
			v2 = val; i2 = idx;
			changed = ffuc_dds_clamp(val, idx, rate, mf, &v2, &i2);
			CHECK(changed == over,
			      "rate %u pitch %.2f: clamp=%d but clock %u cap %llu",
			      rate, pitch, changed, clock,
			      (unsigned long long)cap);
			if (!changed) {
				CHECK(v2 == val && i2 == idx,
				      "rate %u pitch %.2f: unclamped word moved",
				      rate, pitch);
				continue;
			}
			clamped_clock = ffuc_dds_clock(v2, i2, rate);
			CHECK((u64)clamped_clock <= cap,
			      "rate %u pitch %.2f: clamped clock %u above cap %llu",
			      rate, pitch, clamped_clock,
			      (unsigned long long)cap);
			/* Tightest grid point: one 256-unit divisor step below
			 * would exceed the ceiling. */
			{
				u64 p1 = ((u64)v2 << 16) | (i2 & 0xff00);
				u64 k = 0x5f5e10000000ULL;
				unsigned int band = rate > 114000 ? 4 :
						    rate > 57000 ? 2 : 1;
				u64 tighter = (k * band) / (p1 - 256);

				CHECK(tighter > cap,
				      "rate %u pitch %.2f: clamp not tight (%llu)",
				      rate, pitch,
				      (unsigned long long)tighter);
			}
			CHECK((i2 & 0xff) == (idx & 0xff),
			      "rate %u pitch %.2f: family bits changed", rate, pitch);
			CHECK(!ffuc_dds_clamp(v2, i2, rate, mf, &v2, &i2),
			      "rate %u pitch %.2f: clamp not idempotent",
			      rate, pitch);
			CHECK(clamped_clock >= prev_clock,
			      "rate %u pitch %.2f: clock not monotonic", rate, pitch);
			prev_clock = clamped_clock;
		}
	}
}

/* The desk-verified boundary (UCX, 192 kHz, 4x, max_frames 25):
 * +4.1667% is the ceiling; +4.17% programs 200006 Hz and clamps;
 * +4.1% (the slider's 0.1 grid maximum) passes untouched. */
static void test_desk_boundary(void)
{
	u32 val, idx, v2, i2;

	dds_word(192000, 4.1, &val, &idx);
	CHECK(ffuc_dds_clock(val, idx, 192000) == 199872,
	      "192k +4.1%% decode %u", ffuc_dds_clock(val, idx, 192000));
	CHECK(!ffuc_dds_clamp(val, idx, 192000, 25, &v2, &i2),
	      "192k +4.1%% must not clamp");

	dds_word(192000, 4.17, &val, &idx);
	CHECK(ffuc_dds_clock(val, idx, 192000) > 200000,
	      "192k +4.17%% must be over the ceiling");
	CHECK(ffuc_dds_clamp(val, idx, 192000, 25, &v2, &i2),
	      "192k +4.17%% must clamp");
	CHECK(ffuc_dds_clock(v2, i2, 192000) <= 200000,
	      "192k +4.17%% clamped above ceiling");

	/* 176.4k +5% is in-envelope (185220 <= 200000): the 4x band itself
	 * is not the boundary, the capacity is. */
	dds_word(176400, 5.0, &val, &idx);
	CHECK(!ffuc_dds_clamp(val, idx, 176400, 25, &v2, &i2),
	      "176.4k +5%% must not clamp");
	/* 96k +5% (2x): 100800 <= 104000. */
	dds_word(96000, 5.0, &val, &idx);
	CHECK(!ffuc_dds_clamp(val, idx, 96000, 13, &v2, &i2),
	      "96k +5%% must not clamp");
}

/* Degenerate inputs: a zero divisor decodes to 0 and is never clamped. */
static void test_degenerate(void)
{
	u32 v2 = 1, i2 = 1;

	CHECK(ffuc_dds_clock(0, 0, 192000) == 0, "zero divisor decode");
	CHECK(!ffuc_dds_clamp(0, 0, 192000, 25, &v2, &i2),
	      "zero divisor must not clamp");
	CHECK(v2 == 1 && i2 == 1, "zero divisor outputs touched");
}

int main(void)
{
	test_decode_matches_programmed();
	test_clamp_sweep();
	test_desk_boundary();
	test_degenerate();
	if (failures) {
		fprintf(stderr, "pitch-envelope-test: %d failure(s)\n", failures);
		return 1;
	}
	printf("pitch-envelope-test: ok\n");
	return 0;
}