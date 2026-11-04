#!/usr/bin/env bash
# Create the TotalControl 5.1 and 7.1 PipeWire sinks.
#
# The Fireface/MR18 Pro Audio nodes expose 18 playback channels as AUX0..17.
# Desktop players send a 5.1/7.1 stream, which WirePlumber cannot map onto
# those AUX ports, so only FL/FR reach the device.  These two loopback sinks
# give the players real surround layouts and forward the channels to the
# first six/eight SW playback pairs.
set -euo pipefail

RUN_DIR="${XDG_RUNTIME_DIR:-/tmp}/totalcontrol-surround"
PID_FILE="$RUN_DIR/pids"
TARGET_FILE="$RUN_DIR/target"

mkdir -p "$RUN_DIR"

usage() {
    cat <<'EOF'
Usage: totalcontrol-surround.sh [start|stop|status] [target-sink]

  start   create the TotalControl Surround 5.1 and 7.1 sinks
  stop    remove them
  status  show the sinks and their target device

If target-sink is omitted, the default sink is used when it is a
Fireface/MR18 Pro Audio node; otherwise the first matching Pro Audio node
is selected.
EOF
}

pids() {
    if [[ -f "$PID_FILE" ]]; then
        tr '\n' ' ' <"$PID_FILE"
    fi
}

stop_sinks() {
    if [[ -f "$PID_FILE" ]]; then
        while read -r pid; do
            [[ -n "$pid" ]] || continue
            kill "$pid" 2>/dev/null || true
        done <"$PID_FILE"
        rm -f "$PID_FILE"
    fi
    rm -f "$TARGET_FILE"
    # Let the old PipeWire nodes disappear before a new start reuses their
    # names.
    local _i
    for _i in $(seq 1 20); do
        pactl list short sinks 2>/dev/null | grep -q 'totalcontrol-surround' || break
        sleep 0.1
    done
}

target_sink() {
    local explicit="${1:-}"
    if [[ -n "$explicit" ]]; then
        printf '%s\n' "$explicit"
        return 0
    fi
    local default
    default="$(pactl get-default-sink 2>/dev/null || true)"
    if [[ "$default" =~ (Fireface|MR18).*pro-output-0 ]]; then
        printf '%s\n' "$default"
        return 0
    fi
    pactl list short sinks 2>/dev/null |
        awk '{print $2}' |
        grep -E '(Fireface|MR18).*pro-output-0' |
        head -n 1
}

wait_for_output() {
    local node="$1" channel="$2"
    local _i
    for _i in $(seq 1 50); do
        if pw-link -o 2>/dev/null | grep -qx "output.${node}:output_${channel}"; then
            return 0
        fi
        sleep 0.1
    done
    return 1
}

start_sinks() {
    local target
    target="$(target_sink "${1:-}")"
    if [[ -z "$target" ]]; then
        echo "No Fireface/MR18 Pro Audio sink found" >&2
        exit 1
    fi
    if ! pactl list short sinks 2>/dev/null | awk '{print $2}' | grep -qx "$target"; then
        echo "Sink not found: $target" >&2
        exit 1
    fi

    stop_sinks
    : >"$PID_FILE"

    # 5.1: FL FR FC LFE SL SR -> AUX0..AUX5
    pw-loopback -n totalcontrol-surround51 -c 6 \
        -m '[ FL, FR, FC, LFE, SL, SR ]' \
        -i 'media.class=Audio/Sink node.name=totalcontrol-surround51 node.description="TotalControl Surround 5.1"' \
        -P 0 -o 'node.autoconnect=false' &
    echo $! >>"$PID_FILE"

    # 7.1: FL FR FC LFE RL RR SL SR -> AUX0..AUX7
    pw-loopback -n totalcontrol-surround71 -c 8 \
        -m '[ FL, FR, FC, LFE, RL, RR, SL, SR ]' \
        -i 'media.class=Audio/Sink node.name=totalcontrol-surround71 node.description="TotalControl Surround 7.1"' \
        -P 0 -o 'node.autoconnect=false' &
    echo $! >>"$PID_FILE"

    sleep 0.5
    while read -r pid; do
        if ! kill -0 "$pid" 2>/dev/null; then
            echo "A surround sink failed to start" >&2
            stop_sinks
            exit 1
        fi
    done <"$PID_FILE"

    local node channel address
    local idx=0
    for channel in FL FR FC LFE SL SR; do
        node=totalcontrol-surround51
        wait_for_output "$node" "$channel" || {
            echo "Surround 5.1 did not create its ports" >&2
            stop_sinks
            exit 1
        }
        address="playback_AUX$idx"
        pw-link "output.$node:output_$channel" "$target:$address" 2>/dev/null || true
        idx=$((idx + 1))
    done

    idx=0
    for channel in FL FR FC LFE RL RR SL SR; do
        node=totalcontrol-surround71
        wait_for_output "$node" "$channel" || {
            echo "Surround 7.1 did not create its ports" >&2
            stop_sinks
            exit 1
        }
        address="playback_AUX$idx"
        pw-link "output.$node:output_$channel" "$target:$address" 2>/dev/null || true
        idx=$((idx + 1))
    done

    printf '%s\n' "$target" >"$TARGET_FILE"
    echo "TotalControl surround sinks are ready on $target"
    echo "  TotalControl Surround 5.1"
    echo "  TotalControl Surround 7.1"
}

case "${1:-start}" in
    start)
        start_sinks "${2:-}"
        ;;
    restart)
        stop_sinks
        start_sinks "${2:-}"
        ;;
    stop)
        stop_sinks
        echo "TotalControl surround sinks stopped"
        ;;
    status)
        if [[ -f "$TARGET_FILE" ]]; then
            echo "Target: $(cat "$TARGET_FILE")"
        fi
        pactl list short sinks 2>/dev/null | grep 'totalcontrol-surround' || true
        ;;
    -h|--help|help)
        usage
        ;;
    *)
        usage >&2
        exit 2
        ;;
esac
