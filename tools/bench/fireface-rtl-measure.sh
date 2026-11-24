#!/bin/bash
# Fireface physical RTL measurement (UC / UCX, multi-device).
# Stops the PipeWire stack (raw hw access), saves the device scene, routes
# playback AN1/2 -> out5 (by label), measures the chirp round trip with the
# linked-duplex probe, restores the scene, verifies the restore, restarts the
# stack. The scene restore and stack restart run even on error.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
CTL="$HERE/fireface-scene-ctl/target/release/fireface-scene-ctl"

# Detect the Fireface card + model for a device-specific data directory.
CARD=""
for c in 0 1 2 3 4 5 6 7; do
    id=$(cat "/proc/asound/card$c/id" 2>/dev/null || true)
    case "$id" in *Fireface*) CARD=$c; break;; esac
done
if [ -z "$CARD" ]; then echo "no Fireface ALSA card"; exit 1; fi
ID=$(cat "/proc/asound/card$CARD/id")
case "$ID" in
    *UCX*) MODEL=ucx;;
    *UC*)  MODEL=uc;;
    *)     MODEL=fireface;;
esac
DATA="${RTL_DATA:-./rtl-$MODEL}"
SCENE="$DATA/scene.json"
mkdir -p "$DATA"
echo "=== RTL on card $CARD ($ID), model $MODEL -> $DATA"

pw_stop() {
    systemctl --user stop pipewire-pulse.socket pipewire.socket \
        wireplumber.service pipewire-pulse.service pipewire.service 2>/dev/null
    sleep 1
}
pw_start() {
    systemctl --user start pipewire.socket pipewire-pulse.socket \
        pipewire.service pipewire-pulse.service wireplumber.service 2>/dev/null
    sleep 3
}
restore_all() {
    if [ -f "$SCENE" ]; then
        echo "restoring device scene"
        "$CTL" restore "$SCENE"
    fi
    pw_start
    echo "restore done"
}
trap restore_all EXIT
trap 'restore_all; exit 1' INT TERM

pw_stop
"$CTL" save "$SCENE" "$DATA/words.json" || exit 1
"$CTL" route "AN5" 1.0 || exit 1

# rate:period pairs (period = the rate's URB floor, JIT ~1-period fill).
for cfg in "48000 48" "44100 48" "32000 48" "96000 96" "192000 192"; do
    set -- $cfg
    rate=$1; period=$2
    dir="$DATA/r$rate"
    mkdir -p "$dir"
    echo "=== RTL at $rate Hz, period $period"
    if "$HERE/fireface-rtl-probe" "$rate" "$period" 5 "$dir" 4; then
        python3 "$HERE/fireface-rtl-analyze.py" "$dir" | tee "$dir/analysis.txt"
    else
        echo "fireface-rtl-probe failed at $rate" | tee "$dir/analysis.txt"
    fi
done

echo "=== verifying scene restore"
"$CTL" restore "$SCENE"
"$CTL" save "$DATA/scene-after.json"
python3 - "$SCENE" "$DATA/scene-after.json" <<'EOF'
import json, sys
a = json.load(open(sys.argv[1])); b = json.load(open(sys.argv[2]))
a.pop("name", None); b.pop("name", None)
# sample_rate is informational: the device rate follows the host stream, not
# the mixer scene, so it is not part of the routing restore.
for s in (a, b):
    if isinstance(s.get("settings"), dict):
        s["settings"].pop("sample_rate", None)
def diff(x, y, path=""):
    if isinstance(x, dict) and isinstance(y, dict):
        for k in sorted(set(x) | set(y)):
            yield from diff(x.get(k), y.get(k), f"{path}.{k}")
    elif isinstance(x, list) and isinstance(y, list):
        if len(x) != len(y):
            yield f"{path}: len {len(x)} != {len(y)}"
        for i, (u, v) in enumerate(zip(x, y)):
            yield from diff(u, v, f"{path}[{i}]")
    elif x != y:
        yield f"{path}: {x!r} != {y!r}"
d = list(diff(a, b))
if d:
    print("SCENE RESTORE MISMATCH:"); [print(" ", s) for s in d[:20]]
    sys.exit(1)
print("scene restore verified: identical")
EOF
echo "RTL measurement complete"