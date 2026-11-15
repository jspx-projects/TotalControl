#!/bin/sh
# Bench module swap: stop the user audio graph, assert no holder, reload the
# locally built transport module, and optionally restore the graph.
#
#   scripts/bench-reload.sh [--build] [--restore] [insmod args...]
#
# --build    build kernel/ first (matching headers required)
# --restore  start the user audio services again after loading
#
# sudo: uses cached credentials when available, otherwise ksshaskpass.
# The scene and gui.json are untouched: this swaps the module only.
set -eu
cd "$(dirname "$0")/.."

build=0
restore=0
while [ $# -gt 0 ]; do
	case "$1" in
	--build) build=1; shift ;;
	--restore) restore=1; shift ;;
	*) break ;;
	esac
done

if sudo -n dmesg >/dev/null 2>&1; then
	SUDO="sudo"
elif [ -x /usr/bin/ksshaskpass ]; then
	export SUDO_ASKPASS=/usr/bin/ksshaskpass
	SUDO="sudo -A"
else
	echo "sudo needs a password and no askpass helper is available." >&2
	exit 1
fi

UNITS="wireplumber pipewire-pulse pipewire pipewire.socket pipewire-pulse.socket"

if [ "$build" = 1 ]; then
	make -C kernel KBUILD_MODPOST_WARN=1
fi

systemctl --user stop $UNITS 2>/dev/null || true

# Holder assertion: the card must be free before rmmod.  A just-closed
# handle can linger a moment; retry before failing.
i=0
while [ "$i" -lt 10 ]; do
	if ! fuser /dev/snd/* >/dev/null 2>&1; then
		break
	fi
	i=$((i + 1))
	sleep 1
done
if fuser -v /dev/snd/* 2>&1 | grep -q .; then
	echo "HOLDER: /dev/snd still has users:" >&2
	fuser -v /dev/snd/* >&2 || true
	exit 1
fi

if lsmod | grep -q '^snd_fireface_uc '; then
	if ! $SUDO rmmod snd_fireface_uc 2>/dev/null; then
		IFACE=$(ls -d /sys/bus/usb/drivers/snd-fireface-uc/*:*.* 2>/dev/null |
			head -1 | xargs -r -n1 basename)
		if [ -z "$IFACE" ]; then
			echo "module in use and no bound interface to unbind." >&2
			exit 1
		fi
		echo "unbinding $IFACE" >&2
		echo -n "$IFACE" | $SUDO tee /sys/bus/usb/drivers/snd-fireface-uc/unbind >/dev/null
		$SUDO rmmod snd_fireface_uc
	fi
fi

$SUDO modprobe snd-pcm
$SUDO insmod "$PWD/kernel/snd-fireface-uc.ko" "$@"
echo '--- fireface log ---'
$SUDO dmesg | grep -i fireface | tail -3 || true
echo '--- card ---'
aplay -l | grep -A2 -i fireface || true

if [ "$restore" = 1 ]; then
	systemctl --user start pipewire.socket pipewire-pulse.socket \
		wireplumber pipewire pipewire-pulse
fi
