#!/bin/sh
# Compatibility wrapper: the bench module-swap workflow (stop services,
# holder assertion, reload, optional restore) lives in bench-reload.sh.
# Extra arguments are passed to insmod, e.g.
#   ./scripts/reload-kernel.sh urb_frames=48 nurbs=8 ucx_if5=Y
exec "$(dirname "$0")/bench-reload.sh" "$@"
