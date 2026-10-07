#!/bin/sh
# Run from the Mac: build the UI, sync the repo to pitv:~/lumen-tv, build the
# release binary there. Rust builds natively on the Pi (no cross toolchain).
set -e
cd "$(dirname "$0")/.."
npm --prefix ui run build
rsync -a --delete --exclude .git --exclude node_modules --exclude src-tauri/target ./ pitv:lumen-tv/
ssh pitv 'cd lumen-tv/src-tauri && ~/.cargo/bin/cargo build --release'
