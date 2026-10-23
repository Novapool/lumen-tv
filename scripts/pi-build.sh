#!/bin/sh
# Run from the Mac: sync the repo to pitv:~/lumen-tv and build the release
# binary there. Rust builds natively on the Pi (no cross toolchain).
set -e
cd "$(dirname "$0")/.."
# /target is anchored so --delete still clears stale trees (e.g. the old src-tauri/).
rsync -a --delete --exclude .git --exclude /target --exclude /.claude ./ pitv:lumen-tv/
# .git is not synced, so hand the commit to build.rs (the About panel shows it).
GIT=$(git rev-parse --short HEAD 2>/dev/null || echo unknown)
[ -z "$(git status --porcelain 2>/dev/null)" ] || GIT="$GIT-dirty"
ssh pitv "cd lumen-tv && LUMEN_GIT=$GIT ~/.cargo/bin/cargo build --release"
