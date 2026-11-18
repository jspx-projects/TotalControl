#!/usr/bin/env bash
# fb-telemetry-monitor.sh - sample the Fireface transport counters with
# timestamps and append the recent driver kernel log, so audible ticks can
# be correlated with counter deltas and the three-field feedback events.
#
# Usage: sudo ./scripts/fb-telemetry-monitor.sh [seconds] [outfile]
#   seconds  capture duration (default 60)
#   outfile  log file (default ~/Documents/fb-telemetry/fb-telemetry-<ts>.log)
#
# Needs root: /sys/kernel/debug is root-only. Enable the driver's debug log
# first so underruns carry the appl/hw pointers:
#   sudo sh -c 'echo "module snd_fireface_uc +p" > /sys/kernel/debug/dynamic_debug/control'
#
# Read the deltas afterwards: out_underruns = ring ran dry (the tick),
# in_pos_gap = feedback position discontinuity, frames_mismatch = header
# count != packet count. Kernel lines are timestamped; correlate by time.

set -u
SECS="${1:-60}"
TS="$(date +%Y%m%d-%H%M%S)"
REAL_HOME="$HOME"
if [ -n "${SUDO_USER:-}" ]; then
    REAL_HOME="$(getent passwd "$SUDO_USER" | cut -d: -f6)"
fi
OUT="${2:-$REAL_HOME/Documents/fb-telemetry/fb-telemetry-$TS.log}"

if [ "$(id -u)" -ne 0 ]; then
    echo "needs root (debugfs is root-only): run with sudo" >&2
    exit 1
fi

# Find the Fireface card by its ALSA id and derive the USB-interface debugfs
# dir from it (no hardcoded bus/port; survives renumbering and replug).
STATS=""
CARD=""
for c in /sys/class/sound/card*; do
    id="$(cat "$c/id" 2>/dev/null)"
    case "$id" in
        *Fireface*|*Babyface*)
            iface="$(basename "$(readlink "$c/device")")"
            if [ -f "/sys/kernel/debug/$iface/stats" ]; then
                STATS="/sys/kernel/debug/$iface/stats"
                CARD="$(basename "$c")"
                break
            fi
            ;;
    esac
done
if [ -z "$STATS" ]; then
    echo "no Fireface debugfs stats found (module loaded? debugfs mounted?)" >&2
    exit 1
fi

mkdir -p "$(dirname "$OUT")"
echo "monitoring $STATS for ${SECS}s -> $OUT" >&2
{
    echo "# stats: $STATS"
    echo "# start: $(date -Is)  card: $CARD  $(grep rate /proc/asound/$CARD/pcm0p/sub0/hw_params 2>/dev/null)"
} > "$OUT"

END=$(( $(date +%s) + SECS ))
prev=""
while [ "$(date +%s)" -lt "$END" ]; do
    cur="$(grep -E 'out_underruns|in_pos_gap|frames_mismatch|in_overruns|recoveries|fb_ahead|fb_stale|hw_ptr_mismatch|delta_zero' "$STATS" 2>/dev/null)"
    if [ "$cur" != "$prev" ]; then
        echo "[$(date -Is)] CHANGED" >> "$OUT"
        echo "$cur" >> "$OUT"
        prev="$cur"
    fi
    sleep 0.5
done

{
    echo "# end: $(date -Is)"
    echo "# recent driver kernel lines (timestamped):"
    dmesg --time-format iso 2>/dev/null | grep -iE 'fireface|ffuc|snd_fireface' | tail -60
    echo "# feedback gap ring (pos expected delta frames phase wrap):"
    cat "${STATS%stats}gaps" 2>/dev/null
} >> "$OUT"
echo "done -> $OUT" >&2