#!/bin/bash
# audio-load - unified load testing for the Fireface audio path.
#
# Subcommands:
#   audio-load io                          IO load generator (runs until killed)
#   audio-load guard <us> <secs>           hold a PM-QoS CPU-latency constraint
#   audio-load monitor <secs> [outdir]     per-second underrun + scheduling monitor
#   audio-load soak <secs> [opts] [outdir] tone under load, monitored
#   audio-load sweep <secs> [outdir]       graduated CPU load x guard OFF/ON
#
# soak options: --hogs N (CPU busy-loops), --io (IO load), --guard (hold
# cpu_dma_latency=0). The tone is generated with sox; the sink is the Fireface
# Pro Audio output node. Counters come from tools/fireface-hwdep.c (built on demand).
#
# Environment: HWDEP (default /dev/snd/hwC0D0), RATE (default 48000),
# QUANTUM (default 48, the period floor), IO_LOAD_DIR (default $HOME/Documents).
set -u
ROOT=$(cd "$(dirname "$0")/.." && pwd)
HWDEP=${HWDEP:-/dev/snd/hwC0D0}
RATE=${RATE:-48000}
QUANTUM=${QUANTUM:-48}
IO_LOAD_DIR=${IO_LOAD_DIR:-"$HOME/Documents"}
TMP=${TMPDIR:-/tmp}
BIN="$TMP/audio-load-hwdep"
TONE="$TMP/audio-load-tone.wav"

build_bin() {
    [ -x "$BIN" ] && [ "$BIN" -nt "$ROOT/tools/fireface-hwdep.c" ] && return 0
    gcc -O2 -I"$ROOT/kernel" -o "$BIN" "$ROOT/tools/fireface-hwdep.c" || exit 1
}
make_tone() {
    [ -f "$TONE" ] && return 0
    sox -n -r "$RATE" -c 2 "$TONE" synth 120 sine 1000 gain -6 2>/dev/null || {
        echo "sox is required to generate the test tone" >&2; exit 1; }
}
sink_id() {
    pw-dump 2>/dev/null | python3 -c "import json,sys; [print(o['id']) for o in json.load(sys.stdin) if 'pro-output-0' in o.get('info',{}).get('props',{}).get('node.name','')]" | head -1
}
underruns() { "$BIN" counters "$HWDEP" 2>/dev/null | sed -n 's/^out_underruns=\([0-9]*\).*/\1/p'; }
card_no() { local c=${HWDEP#*hwC}; echo "${c%D*}"; }
fill() { awk '/^hw_ptr/{h=$3} /^appl_ptr/{a=$3} END{printf "%d", a-h}' "/proc/asound/card$(card_no)/pcm0p/sub0/status" 2>/dev/null; }

start_load() {  # $1=hogs $2=io(0/1) $3=guard(0/1) $4=secs ; sets LOAD_PIDS
    LOAD_PIDS=()
    local n=$1 io=$2 guard=$3 secs=$4 c
    if [ "$guard" = 1 ]; then
        ( exec 3>/dev/cpu_dma_latency; printf '\x00\x00\x00\x00' >&3; sleep "$((secs + 20))" ) &
        LOAD_PIDS+=($!)
    fi
    for c in $(seq 0 $((n - 1))); do
        taskset -c "$c" bash -c 'while :; do :; done' >/dev/null 2>&1 &
        LOAD_PIDS+=($!)
    done
    if [ "$io" = 1 ]; then
        ( while :; do find "$IO_LOAD_DIR" -type f -printf '%s\n' >/dev/null 2>&1; sleep 2; done ) &
        LOAD_PIDS+=($!)
    fi
}
stop_load() { local p; for p in "${LOAD_PIDS[@]:-}"; do kill "$p" 2>/dev/null; done; for p in "${LOAD_PIDS[@]:-}"; do wait "$p" 2>/dev/null; done; }

start_tone() {  # $1=sink ; sets TONE_PID
    ( while :; do pw-play --target "$1" "$TONE" >/dev/null 2>&1; done ) &
    TONE_PID=$!
}
stop_tone() { kill "$TONE_PID" 2>/dev/null; pkill -x pw-play 2>/dev/null; }

monitor() {  # $1=secs $2=outdir
    local secs=$1 out=$2 pwpid tid prev_u u f now
    mkdir -p "$out"
    pwpid=$(pgrep -x pipewire | head -1)
    tid=$(ps -T -p "$pwpid" -o tid=,comm= 2>/dev/null | awk '$2=="data-loop.0"{print $1}')
    prev_u=$(underruns); prev_u=${prev_u:-0}
    echo "epoch underruns fill delay_ms" > "$out/soak.log"
    echo "baseline underruns $prev_u (pipewire pid $pwpid tid $tid)" | tee "$out/meta.txt"
    local prev_delay=0 prev_run=0
    [ -n "$tid" ] && read -r prev_run prev_delay _ < "/proc/$pwpid/task/$tid/schedstat"
    for _ in $(seq 1 "$secs"); do
        sleep 1
        now=$(date +%s.%N)
        u=$(underruns); u=${u:-0}
        f=$(fill)
        local dd=0 dr=0
        if [ -n "$tid" ]; then
            local run delay slices
            read -r run delay slices < "/proc/$pwpid/task/$tid/schedstat"
            dd=$(( (delay - prev_delay) / 1000000 )); dr=$(( (run - prev_run) / 1000000 ))
            prev_delay=$delay; prev_run=$run
        fi
        printf '%s underruns=%s fill=%s delay_ms=%s run_ms=%s\n' "$now" "$u" "$f" "$dd" "$dr" >> "$out/soak.log"
        if [ "$u" -gt "$prev_u" ]; then
            local ts; ts=$(date +%H%M%S)
            sudo -n ff-lab ringtrace > "$out/trace-$ts.txt" 2>/dev/null
            sudo -n ff-lab stats > "$out/stats-$ts.txt" 2>/dev/null
            echo "UNDERRUN at $now (count $u) -> trace-$ts.txt" | tee -a "$out/events.txt"
        fi
        prev_u=$u
    done
    echo "done: $(grep -c UNDERRUN "$out/events.txt" 2>/dev/null || echo 0) underruns in ${secs}s"
}

cmd=${1:-}; shift || true
case "$cmd" in
io)
    echo "IO load on $IO_LOAD_DIR (kill to stop)"
    while :; do find "$IO_LOAD_DIR" -type f -printf '%s\n' >/dev/null 2>&1; sleep 2; done
    ;;
guard)
    us=${1:-0}; secs=${2:-60}
    ( exec 3>/dev/cpu_dma_latency
      python3 -c "import os,sys; os.write(3, int(sys.argv[1]).to_bytes(4,'little'))" "$us"
      echo "latency $us us held for $secs s"; sleep "$secs" )
    ;;
monitor)
    build_bin
    secs=${1:-60}; out=${2:-"$ROOT/docs/work/bench/data/monitor"}
    monitor "$secs" "$out"
    ;;
soak)
    build_bin; make_tone
    secs=60; hogs=0; io=0; guard=0; out=""
    while [ $# -gt 0 ]; do case "$1" in
        --hogs) hogs=$2; shift 2;; --io) io=1; shift;; --guard) guard=1; shift;;
        *) if [ -z "$out" ] && [[ "$1" =~ ^[0-9]+$ ]]; then secs=$1; else out=$1; fi; shift;;
    esac; done
    out=${out:-"$ROOT/docs/work/bench/data/soak"}
    sink=$(sink_id); [ -n "$sink" ] || { echo "no Fireface sink"; exit 1; }
    pw-metadata -n settings 0 clock.force-rate "$RATE" >/dev/null 2>&1
    pw-metadata -n settings 0 clock.force-quantum "$QUANTUM" >/dev/null 2>&1
    trap 'stop_load; stop_tone' EXIT
    start_tone "$sink"; sleep 2
    start_load "$hogs" "$io" "$guard" "$secs"; sleep 2
    echo "soak ${secs}s: hogs=$hogs io=$io guard=$guard rate=$RATE quantum=$QUANTUM" | tee "$out/run.txt"
    monitor "$secs" "$out"
    ;;
sweep)
    build_bin; make_tone
    secs=${1:-30}; out=${2:-"$ROOT/docs/work/bench/data/load-char"}
    mkdir -p "$out"
    sink=$(sink_id); [ -n "$sink" ] || { echo "no Fireface sink"; exit 1; }
    pw-metadata -n settings 0 clock.force-rate "$RATE" >/dev/null 2>&1
    pw-metadata -n settings 0 clock.force-quantum "$QUANTUM" >/dev/null 2>&1
    trap 'stop_load; stop_tone' EXIT
    start_tone "$sink"; sleep 3
    echo "sweep rate=$RATE quantum=$QUANTUM ${secs}s/level, IO load on" | tee "$out/table.txt"
    echo "hogs guards underruns" | tee -a "$out/table.txt"
    for n in 0 2 4 8 12 16; do
        for g in 0 1; do
            start_load "$n" 1 "$g" "$secs"; sleep 3
            u0=$(underruns); sleep "$secs"; u1=$(underruns)
            printf '%s %s %s\n' "$n" "$([ "$g" = 1 ] && echo on || echo off)" "$(( ${u1:-0} - ${u0:-0} ))" | tee -a "$out/table.txt"
            stop_load; sleep 2
        done
    done
    sudo -n ff-lab stats 2>/dev/null | grep -E "ring_max_stall|ring_starved|ring_min_filled" | tee -a "$out/table.txt"
    ;;
*)
    grep '^#' "$0" | sed -n '2,12p'
    exit 2
    ;;
esac
