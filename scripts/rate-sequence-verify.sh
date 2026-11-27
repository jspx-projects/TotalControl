#!/bin/sh
# Verify the ISO rate sequence against the RME USB audio engine.
#
# The expected sequence on every engine start is one DDS word (0x1b) for
# the target base family, then the family selector (0x10/0x30).  The options
# word (0x10/0x5cf) is not part of the sequence.  A host-written DDS word
# (varispeed) is re-sent on the next start; a nominal word is used when the
# family has no host word yet.
#
# The check reads the driver counters before and after direct-ALSA duplex
# starts and compares the actual device clock (debugfs rate_meas).  It leaves
# the device on the last requested rate.  A USB-fault recovery is not
# forceable from the host without a reset (which re-probes the unit); the
# recover work enters the same ff_engine_start commit path verified here.
#
#   scripts/rate-sequence-verify.sh [rate ...]
#
# Environment:
#   FIREFFACE_HWDEP   fireface-hwdep binary (default: tools/fireface-hwdep)
#   ALSA_PROBE        alsa-cushion-probe binary (default: tools/alsa-cushion-probe)
#   CARD              ALSA card number or id (default: Fireface)
#   NODE              hwdep node (default: autodetect)
#   START_SECONDS     stream length per case (default: 3)
set -u
cd "$(dirname "$0")/.."

HWDEP=${FIREFFACE_HWDEP:-tools/fireface-hwdep}
PROBE=${ALSA_PROBE:-tools/alsa-cushion-probe}
CARD=${CARD:-Fireface}
SECS=${START_SECONDS:-3}

fail=0
pass=0

say() { printf '%s\n' "$*"; }
ok() { pass=$((pass + 1)); say "PASS  $*"; }
bad() { fail=$((fail + 1)); say "FAIL  $*"; }

[ -x "$HWDEP" ] || { say "missing $HWDEP (build: gcc -O2 -I kernel -o tools/fireface-hwdep tools/fireface-hwdep.c)"; exit 2; }
[ -x "$PROBE" ] || { say "missing $PROBE (build: make -C tools alsa-cushion-probe or cc -O2 -o ... tools/alsa-cushion-probe.c -lasound)"; exit 2; }

if [ -z "${NODE:-}" ]; then
	for n in /dev/snd/hwC*D*; do
		[ -e "$n" ] || continue
		if "$HWDEP" info "$n" 2>/dev/null | grep -q 'transport=1'; then
			NODE=$n
			break
		fi
	done
fi
[ -n "${NODE:-}" ] || { say "no ISO Fireface hwdep node found"; exit 2; }

counter() { "$HWDEP" counters "$NODE" 2>/dev/null | sed -n "s/^$1=//p"; }
snap() {
	for c in dds_words family_writes rate_commits option_writes; do
		eval "before_$c=\$(counter $c)"
	done
}
delta() {
	eval "before=\$before_$1"
	echo $(( $(counter "$1") - before ))
}

clock_field() { sudo -n /usr/local/sbin/ffuc-lab clock 2>/dev/null | awk -v k="$1" '$1 == k {print $2}'; }

# One duplex start at $1.  Reads the actual clock mid-stream.  Leaves the
# counter deltas in d_dds, d_fam, d_commit and d_opt.
run_start() {
	rate=$1
	shift
	[ $# -eq 0 ] && set -- -1 0 0
	probe_rc=0
	snap
	"$PROBE" "$CARD" "$rate" 48 48 "$SECS" 768 "$@" >/dev/null 2>&1 &
	probe_pid=$!
	sleep 2.2
	actual=$(clock_field rate_meas)
	wait "$probe_pid" || probe_rc=$?
	d_dds=$(delta dds_words)
	d_fam=$(delta family_writes)
	d_commit=$(delta rate_commits)
	d_opt=$(delta option_writes)
	probe_rc=${probe_rc:-0}
}

# A clean start: exactly one DDS, one family write, one commit, no options.
expect_start() {
	label=$1
	rate=$2
	[ "$d_dds" -eq 1 ] && [ "$d_fam" -eq 1 ] && [ "$d_commit" -eq 1 ] \
		&& ok "$label: counters dds=$d_dds family=$d_fam commits=$d_commit" \
		|| bad "$label: dds=$d_dds family=$d_fam commits=$d_commit (expected 1/1/1)"
	[ "$d_opt" -eq 0 ] \
		&& ok "$label: no options commit" \
		|| bad "$label: $d_opt options write(s) in the start path"
	if [ -n "${actual:-}" ] && [ "$actual" -gt 0 ]; then
		d=$((actual - rate)); [ "$d" -lt 0 ] && d=$((-d))
		[ "$d" -le 100 ] \
			&& ok "$label: rate_meas $actual for $rate Hz" \
			|| bad "$label: rate_meas $actual for $rate Hz"
	else
		bad "$label: no rate_meas during the start"
	fi
	[ "$probe_rc" -eq 0 ] \
		&& ok "$label: stream clean" \
		|| bad "$label: probe reported a stream fault (rc=$probe_rc)"
}

# One start's counter invariant, for cases whose clock is intentionally
# pitched and cannot be compared with the nominal rate.
expect_words() {
	label=$1
	[ "$d_dds" -eq 1 ] && [ "$d_fam" -eq 1 ] && [ "$d_commit" -eq 1 ] \
		&& ok "$label: counters dds=$d_dds family=$d_fam commits=$d_commit" \
		|| bad "$label: dds=$d_dds family=$d_fam commits=$d_commit (expected 1/1/1)"
	[ "$d_opt" -eq 0 ] \
		&& ok "$label: no options commit" \
		|| bad "$label: $d_opt options write(s) in the start path"
}

for rate in "$@"; do
	say "=== start $rate Hz"
	run_start "$rate"
	expect_start "start $rate" "$rate"
done

# Default matrix when no rates are given: same-rate repeat, change, repeat,
# change back, and a 2x band.
if [ $# -eq 0 ]; then
	say "=== same-rate repeat 48000"
	run_start 48000; expect_start "48000 #1" 48000
	run_start 48000; expect_start "48000 #2" 48000
	say "=== change to 44100"
	run_start 44100; expect_start "44100 #1" 44100
	run_start 44100; expect_start "44100 #2" 44100
	say "=== change back to 48000"
	run_start 48000; expect_start "48000 #3" 48000
	say "=== 2x band 96000"
	run_start 96000; expect_start "96000" 96000
	say "=== change back to 44100"
	run_start 44100; expect_start "44100 #3" 44100

	say "=== raw family select, then start 48000"
	"$HWDEP" vw 0x10 0x10 0x30 "$NODE" >/dev/null
	run_start 48000; expect_start "raw family then 48000" 48000

	say "=== host pitch survives a start"
	# 48 kHz family at +1 %: DDS 0x5f5e10000000 / 48480.
	"$HWDEP" vw 0x1b 0x80eb 0x4c02 "$NODE" >/dev/null
	run_start 48000
	expect_words "pitch start"
	if [ -n "${actual:-}" ] && [ "$actual" -ge 48400 ] && [ "$actual" -le 48560 ]; then
		ok "pitch start: rate_meas $actual keeps the +1 % clock"
	else
		bad "pitch start: rate_meas ${actual:-none} lost the +1 % clock"
	fi
	say "=== pitch clamp across a band change"
	# 48 kHz family at +5 %: 50400 Hz at 1x, 201600 Hz at 4x.  The 4x
	# transport ceiling is 200000 Hz, so the start at 192 kHz must clamp.
	"$HWDEP" vw 0x1b 0x7c02 0x0802 "$NODE" >/dev/null
	run_start 48000
	expect_words "pitch clamp 48k"
	if [ -n "${actual:-}" ] && [ "$actual" -ge 50300 ] && [ "$actual" -le 50500 ]; then
		ok "pitch clamp: rate_meas $actual at 48 kHz"
	else
		bad "pitch clamp: rate_meas ${actual:-none} at 48 kHz"
	fi
	clamps_before=$(counter pitch_clamps)
	run_start 192000
	expect_words "pitch clamp 192k"
	clamps=$(( $(counter pitch_clamps) - clamps_before ))
	if [ -n "${actual:-}" ] && [ "$actual" -ge 198000 ] && [ "$actual" -le 200100 ]; then
		ok "pitch clamp: rate_meas $actual inside the 4x ceiling"
	else
		bad "pitch clamp: rate_meas ${actual:-none} outside the 4x ceiling"
	fi
	[ "$clamps" -ge 1 ] \
		&& ok "pitch clamp: clamp counted ($clamps)" \
		|| bad "pitch clamp: clamp not counted"

	# Restore the nominal words, then leave the unit on the scene rate.
	"$HWDEP" vw 0x1b 0xc350 0x0000 "$NODE" >/dev/null
	"$HWDEP" vw 0x1b 0x8db9 0x2d01 "$NODE" >/dev/null
	"$HWDEP" vw 0x1b 0x8235 0x5502 "$NODE" >/dev/null
	"$HWDEP" vw 0x1b 0x7d00 0x0003 "$NODE" >/dev/null
	run_start 48000; expect_start "nominal restored 48000" 48000
	say "=== leave on 44100"
	run_start 44100; expect_start "final 44100" 44100
fi

say "=== done: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
