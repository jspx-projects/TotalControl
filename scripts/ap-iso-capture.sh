#!/usr/bin/env bash
# ap-iso-capture.sh - capture the Fireface AP/ISO USB traffic (IN headers and
# OUT lead words) via usbmon while audio plays.
#
# Usage: sudo bash scripts/ap-iso-capture.sh [seconds] [outfile.pcap]
#   Play audio (music or a test tone) during the capture window.
#
# Needs root (usbmon). The capture is saved outside the repository.
# Analyse afterwards with tshark: ISO OUT packets carry the lead words,
# ISO IN packets carry the position/count/phase header.

set -u
SECS="${1:-15}"
TS=$(date +%Y%m%d-%H%M%S)
REAL_HOME="$HOME"
if [ -n "${SUDO_USER:-}" ]; then
    REAL_HOME=$(getent passwd "$SUDO_USER" | cut -d: -f6)
fi
OUT="${2:-$REAL_HOME/Documents/fb-telemetry/ap-iso-$TS.pcap}"

uid=$(id -u)
if [ "$uid" -ne 0 ]; then
    echo "needs root (usbmon): run with sudo" >&2
    exit 1
fi

CARD=""
for c in /sys/class/sound/card*; do
    cid=$(cat "$c/id" 2>/dev/null)
    case "$cid" in
        *Fireface*) CARD=$(basename "$c"); break ;;
    esac
done
if [ -z "$CARD" ]; then
    echo "no Fireface card found" >&2
    exit 1
fi
DEV=$(readlink "/sys/class/sound/$CARD/device")
BUS=$(basename "$DEV" | cut -d- -f1)
OUTDIR=$(dirname "$OUT")
mkdir -p "$OUTDIR"
echo "capturing usbmon$BUS for ${SECS}s -> $OUT  (play audio now)" >&2
timeout "$SECS" tcpdump -i "usbmon$BUS" -s 0 -w "$OUT" 2>/dev/null
if [ -n "${SUDO_USER:-}" ]; then
    grp=$(id -gn "$SUDO_USER")
    chown "$SUDO_USER:$grp" "$OUT" 2>/dev/null
fi
echo "done -> $OUT" >&2
ls -la "$OUT" >&2