#!/bin/bash
HERE=$(cd "$(dirname "$0")" && pwd)
# Underrun soak monitor: per-second counters + refill-thread scheduling
# latency; dumps the driver ring trace right after each underrun.
#   underrun-soak.sh <seconds> <outdir>     (CARD=N selects the card, default 0)
set -u
secs=${1:-120}
out=${2:-"./soak"}
mkdir -p "$out"
diag="$HERE/ff-diag"
CARD=${CARD:-0}
node=/dev/snd/hwC${CARD}D0

# Find the PipeWire graph thread (data-loop.0).
pwpid=$(pgrep -x pipewire | head -1)
tid=$(ps -T -p "$pwpid" -o tid=,comm= | awk '$2=="data-loop.0"{print $1}' | head -1)
echo "pipewire pid $pwpid data-loop tid $tid" | tee "$out/meta.txt"
date +%s.%N > "$out/start.ts"

schedstat() { cat "/proc/$pwpid/task/$tid/schedstat" 2>/dev/null; }
counters() { "$diag" "$node" 2>/dev/null | awk '/^(out_underruns|rate_mismatch|out_pkt_status|recoveries|in_overruns)/{printf "%s=%s ", $1, $2}'; }
fill() { awk '/^hw_ptr/{h=$3} /^appl_ptr/{a=$3} END{printf "%d", a-h}' /proc/asound/card$CARD/pcm0p/sub0/status 2>/dev/null; }

prev_u=0; prev_delay=0; prev_run=0
# Baseline the counters so the first sample is not a false event.
prev_u=$(counters | sed -n 's/.*out_underruns=\([0-9]*\).*/\1/p')
read -r prev_run prev_delay _ <<< "$(schedstat)"
echo "baseline underruns $prev_u" | tee -a "$out/meta.txt"
echo "epoch underruns rate_mismatch pkt_status recov fill run_delay_ms delta_delay_ms slices" > "$out/soak.log"
for i in $(seq 1 "$secs"); do
    sleep 1
    now=$(date +%s.%N)
    c=$(counters)
    u=$(echo "$c" | sed -n 's/.*out_underruns=\([0-9]*\).*/\1/p')
    f=$(fill)
    read -r run delay slices <<< "$(schedstat)"
    dd=$(( (delay - prev_delay) / 1000000 ))
    dr=$(( (run - prev_run) / 1000000 ))
    printf '%s %s fill=%s delay_ms=%s run_ms=%s slices=%s\n' "$now" "$c" "$f" "$dd" "$dr" "$slices" >> "$out/soak.log"
    if [ "${u:-0}" -gt "$prev_u" ]; then
        ts=$(date +%H%M%S)
        sudo -n ff-lab ringtrace > "$out/trace-$ts.txt" 2>/dev/null || echo "ringtrace unavailable" > "$out/trace-$ts.txt"
        sudo -n ff-lab stats > "$out/stats-$ts.txt" 2>/dev/null
        echo "UNDERRUN at $now (count $u) -> trace-$ts.txt" | tee -a "$out/events.txt"
    fi
    prev_u=${u:-0}; prev_delay=$delay; prev_run=$run
done
echo "done: $(grep -c UNDERRUN "$out/events.txt" 2>/dev/null || echo 0) underruns in ${secs}s"