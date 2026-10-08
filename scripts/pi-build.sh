#!/bin/sh
# Run from the Mac: sync the repo to pitv:~/lumen-tv and build the release
# binary there. Rust builds natively on the Pi (no cross toolchain).
set -e
cd "$(dirname "$0")/.."
# /target is anchored so --delete still clears stale trees (e.g. the old src-tauri/).
rsync -a --delete --exclude .git --exclude /target ./ pitv:lumen-tv/
ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo build --release'
