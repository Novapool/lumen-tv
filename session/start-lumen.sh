#!/bin/sh
# Run by labwc (-S) inside the Lumen session. Sets the TV to 1080p60 first:
# WebKit can't animate at 4K on the Pi 5 (M1 perf results), and the Pi decodes
# video above 1080p in software anyway. The TV upscales.
OUT="${LUMEN_OUTPUT:-HDMI-A-1}"
MODE="${LUMEN_MODE:-1920x1080@60}"
wlr-randr --output "$OUT" --mode "$MODE" || wlr-randr --output "$OUT" --mode "${MODE%@*}" \
  || echo "start-lumen: could not set $MODE on $OUT, staying at the current mode"
wlr-randr | grep -E "^[^ ]|current" # log what we ended up with
exec "$HOME/lumen-tv/src-tauri/target/release/lumen-tv"
