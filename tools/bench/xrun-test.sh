#!/bin/bash
HERE=$(cd "$(dirname "$0")" && pwd)
# Controlled starvation: freeze the PipeWire graph briefly, then measure
# the driver's grace/XRUN behaviour and PipeWire's auto-recovery time.
#   xrun-test.sh <outdir>        (CARD=N selects the card, default 0)
set -u
CARD=${CARD:-0}
out=${1:-"./xrun-test"}
mkdir -p "$out"
pwpid=$(pgrep -x pipewire | head -1)
state() { awk '/^state/{print $2}' /proc/asound/card$CARD/pcm0p/sub0/status; }
sudo -n ffuc-lab stats > "$out/stats-before.txt"
echo "before: $(state)"
kill -STOP "$pwpid"
sleep 0.1
kill -CONT "$pwpid"
echo "continued at $(date +%s.%N)"
for i in $(seq 1 100); do
    s=$(state)
    printf '%s %s\n' "$(date +%s.%N)" "$s" >> "$out/recovery.log"
    [ "$s" = "RUNNING" ] && [ "$i" -gt 2 ] && break
    sleep 0.1
done
echo "after: $(state) at $(date +%s.%N)"
sudo -n ffuc-lab ringtrace > "$out/trace.txt"
sudo -n ffuc-lab stats > "$out/stats-after.txt"
grep -E "out_underruns|recoveries|ring_" "$out/stats-after.txt"