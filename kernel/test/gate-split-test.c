// SPDX-License-Identifier: GPL-2.0-only
// Host test of the duplex gate split invariant.
#include <stdio.h>
#include <stdbool.h>
#include "../ff-gate.h"

int main(void)
{
	if (!ff_gate_together(128, 128)) {
		fprintf(stderr, "equal periods must gate together\n");
		return 1;
	}
	if (ff_gate_together(127, 128)) {
		fprintf(stderr, "unequal periods must not gate together\n");
		return 1;
	}
	if (ff_gate_together(0, 0)) {
		fprintf(stderr, "unknown periods must not gate together\n");
		return 1;
	}
	puts("gate-split-test: ok");
	return 0;
}
