#!/bin/sh
# Verify the UCX interrupt (IF5) rate change against the Windows captures.
#
# Windows UCX real rate changes (E8/E9, win-20261027-ucx-transport) write
# only the family selector 0x10/0x30.  They do not rewrite the DDS quad and
# do not write the options commit.  The four-word DDS quad is written at
# connect and on a settings apply.
#
# The check drives direct-ALSA duplex starts and reads the driver counters.
# The IF5 transport has no usable rate_meas (the ISO timestamp header is not
# present), so the selected rate is read from the state word 0x11, whose
# high byte is the rate index.
#
#   scripts/int-rate-verify.sh [rate ...]
#
# This script requires the module loaded with ucx_if5=Y.  It aborts on an
# ISO node.
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

[ -x "$HWDEP" ] || { say "missing $HWDEP"; exit 2; }
[ -x "$PROBE" ] || { say "missing $PROBE"; exit 2; }

if [ -z "${NODE:-}" ]; then
	for n in /dev/snd/hwC*D*; do
		[ -e "$n" ] || continue
		if "$HWDEP" info "$n" 2>/dev/null | grep -q 'transport=0'; then
			NODE=$n
			break
		fi
	done
fi
[ -n "${NODE:-}" ] || { say "no interrupt Fireface hwdep node found (load with ucx_if5=Y)"; exit 2; }
"$HWDEP" info "$NODE" | grep -q 'transport=0' || { say "$NODE is not the interrupt transport"; exit 2; }

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
state_index() {
	word=$("$HWDEP" read 0x11 "$NODE" 2>/dev/null | awk '{print $4}')
	[ -n "$word" ] || { echo ""; return; }
	echo $(( (0x$word >> 24) & 0xff ))
}
expected_index() {
	case "$1" in
		32000) echo 0 ;;
		44100) echo 1 ;;
		48000) echo 2 ;;
		64000) echo 4 ;;
		88200) echo 5 ;;
		96000) echo 6 ;;
		128000) echo 8 ;;
		176400) echo 9 ;;
		192000) echo 10 ;;
		*) echo -1 ;;
	esac
}

run_start() {
	rate=$1
	probe_rc=0
	snap
	# IF5 period floor is 1024 frames; prefill two periods.
	"$PROBE" "$CARD" "$rate" 48 48 "$SECS" 2048 -1 0 0 >/dev/null 2>&1 &
	probe_pid=$!
	sleep 2.0
	idx=$(state_index)
	wait "$probe_pid" || probe_rc=$?
	d_dds=$(delta dds_words)
	d_fam=$(delta family_writes)
	d_commit=$(delta rate_commits)
	d_opt=$(delta option_writes)
	rc=${probe_rc:-0}
}

expect_int_start() {
	label=$1
	rate=$2
	want=$3
	[ "$d_dds" -eq 0 ] && [ "$d_fam" -eq 1 ] && [ "$d_commit" -eq 1 ] \
		&& ok "$label: counters dds=$d_dds family=$d_fam commits=$d_commit" \
		|| bad "$label: dds=$d_dds family=$d_fam commits=$d_commit (expected 0/1/1)"
	[ "$d_opt" -eq 0 ] \
		&& ok "$label: no options commit" \
		|| bad "$label: $d_opt options write(s) in the rate change"
	[ "$idx" = "$want" ] \
		&& ok "$label: state rate index $idx" \
		|| bad "$label: state rate index ${idx:-none}, expected $want"
	if [ "$rc" -eq 0 ]; then
		ok "$label: stream clean"
	else
		bad "$label: probe reported a stream fault (rc=$rc)"
	fi
}

if [ $# -eq 0 ]; then
	set -- 48000 48000 44100 44100 48000 44100
fi

for rate in "$@"; do
	say "=== start $rate Hz"
	run_start "$rate"
	expect_int_start "start $rate" "$rate" "$(expected_index "$rate")"
done

say "=== done: $pass passed, $fail failed"
[ "$fail" -eq 0 ]
