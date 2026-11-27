#!/bin/bash
# Cushion matrix: stall duration x write phase x ring-fill target, direct ALSA.
# The probe bypasses PipeWire and TotalControl, so stop them or let their
# nodes suspend before running. Requires the lab `ff-lab stats` helper for
# the driver's ring counters.
#
#   scripts/cushion-matrix.sh [out.csv]
set -u
ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
OUT=${1:-"$ROOT/cushion-matrix.csv"}
PROBE=${PROBE:-"$ROOT/tools/alsa-cushion-probe"}
RATE=${RATE:-44100}
PERIOD=${PERIOD:-1024}
TARGETS=${TARGETS:-"1280 3072 4096 5120"}
STALLS=${STALLS:-"50 60 70 80"}
TRIALS=${TRIALS:-2}

if [ ! -x "$PROBE" ]; then
    cc -O2 -Wall -Wextra -Werror -o "$PROBE" \
        "$ROOT/tools/alsa-cushion-probe.c" -lasound || exit 1
fi
CARD=$(awk '/Fireface|Babyface/{print $1}' /proc/asound/cards | tr -d '[]' | head -1)
[ -n "$CARD" ] || { echo "no Fireface card found" >&2; exit 1; }
STATS() { sudo -n /usr/local/sbin/ff-lab stats 2>/dev/null; }
num() { printf '%s\n' "$1" | awk -v k="$2" '$1==k{print $2}'; }

echo "target phase stall_ms trial out_delta ring_min starved app_xruns stall_fill" > "$OUT"
for target in $TARGETS; do
    for phase in 1 2; do
        for stall in $STALLS; do
            for trial in $(seq 1 "$TRIALS"); do
                before=$(STATS)
                ou0=$(num "$before" out_underruns)
                out=$(chrt -f 50 "$PROBE" "$CARD" "$RATE" "$PERIOD" "$PERIOD" \
                    5 "$target" 1500 "$stall" "$phase" 2>&1)
                app_x=$(printf '%s\n' "$out" | sed -n 's/.*app_xruns=\([0-9]*\).*/\1/p')
                sf=$(printf '%s\n' "$out" | sed -n 's/.*stall_fill=\(-\?[0-9]*\).*/\1/p')
                after=$(STATS)
                ou1=$(num "$after" out_underruns)
                rmin=$(num "$after" ring_min_filled)
                starved=$(num "$after" ring_starved)
                echo "$target $phase $stall $trial $((ou1-ou0)) $rmin $starved ${app_x:-?} ${sf:-?}" >> "$OUT"
                printf '%s %s %s %s: underruns %s ring_min %s starved %s fill %s\n' \
                    "$target" "$phase" "$stall" "$trial" "$((ou1-ou0))" "$rmin" "$starved" "${sf:-?}"
            done
        done
    done
done
echo "wrote $OUT"
