#!/bin/bash
# IO load generator: baloo-like indexing pressure (page cache + disk reads).
# Runs until killed; pair with the soak monitor.
#   io-load.sh [dir ...]      (default: "$HOME/Documents" "$HOME/.local")
dirs=("$@")
if [ ${#dirs[@]} -eq 0 ]; then
    dirs=("$HOME/Documents" "$HOME/.local")
fi
while :; do
    find "${dirs[@]}" -type f -printf '%s\n' > /dev/null 2>&1
    sleep 2
done