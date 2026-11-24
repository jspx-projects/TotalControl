#!/bin/bash
# Reproducible UCX loopback measurement, using the consolidated tools.
# See docs/DEVELOPMENT-TOOLS.md "Loopback measurement (UCX)".
#
#   loopback-measure.sh <out-pair> <pbN|inN> <internal|cable> [rate] [seconds]
#   e.g. loopback-measure.sh AN5/6 pb1 internal 48000 2
#
# Requires the GUI closed (shared hwdep FIFO + scene reapply). Frees the card,
# configures routing with fireface-hwdep loopback-setup, measures with
# alsa-latency-probe, restores the profile on exit.
set -u
HERE=$(cd "$(dirname "$0")" && pwd)
# fireface-hwdep and alsa-latency-probe live in tools/ (the parent directory).
HWDEP="${HWDEP:-$HERE/../fireface-hwdep}"
PROBE="${PROBE:-$HERE/../alsa-latency-probe}"
DATA="${LOOPBACK_DATA:-./loopback-measure}"
mkdir -p "$DATA"

OUT="${1:-AN5/6}"; SRC="${2:-pb1}"; MODE="${3:-internal}"; RATE="${4:-48000}"; SECS="${5:-2}"
if pgrep -f "release/totalcontrol-gui" >/dev/null; then
    echo "ABORT: close the TotalControl GUI first (shared hwdep FIFO + scene reapply)." >&2
    exit 1
fi
CARD_NAME=$(pactl list cards short | awk '/Fireface/ {print $2; exit}')
[ -n "$CARD_NAME" ] || { echo "ABORT: no Fireface card" >&2; exit 1; }
CARD=$(awk '/Fireface/ {print $1; exit}' /proc/asound/cards)

restore() { pactl set-card-profile "$CARD_NAME" pro-audio 2>/dev/null; echo "profile restored"; }
trap restore EXIT
trap 'exit 1' TERM INT

pactl set-card-profile "$CARD_NAME" off
for i in $(seq 1 20); do grep -q closed /proc/asound/card$CARD/pcm0p/sub0/status && break; sleep 0.5; done

CABLE=""; [ "$MODE" = cable ] && CABLE="--cable"
"$HWDEP" loopback-setup "$OUT" "$SRC" $CABLE /dev/snd/hwC${CARD}D0 || exit 1

# Capture channels = same-numbered input as the output pair.
case "$OUT" in
    AN1/2) CA=1; CB=2;; AN3/4) CA=3; CB=4;; AN5/6) CA=5; CB=6;;
    PH7/8) CA=7; CB=8;; SPDIF) CA=9; CB=10;; ADAT1/2) CA=11; CB=12;;
    ADAT3/4) CA=13; CB=14;; ADAT5/6) CA=15; CB=16;; ADAT7/8) CA=17; CB=18;;
    *) echo "unknown out pair" >&2; exit 2;;
esac
PA=${SRC#pb}; PA=${PA#in}
PB=$((PA + 1))

if [ "$MODE" = cable ]; then
    echo "== cable impulse latency (play ch$PA, cap ch$CA)"
    "$PROBE" hw:$CARD,0 "$RATE" 1024 48 $((PA - 1)) $((CA - 1)) -1 18 0.5
else
    echo "== internal loopback, sample-exact pattern"
    DUMP="$DATA/${OUT//\//_}-${SRC}-${MODE}.f32" \
        "$PROBE" pattern hw:$CARD,0 "$RATE" "$SECS" "$PA" "$PB" "$CA" "$CB" 18
fi