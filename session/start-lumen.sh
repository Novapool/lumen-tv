#!/bin/sh
# Run by labwc (-S) inside the Lumen session. Sets the TV to 1080p60 first:
# no UI toolkit holds 60 fps at 4K on the Pi 5 GPU (fill-rate bound, see
# MILESTONES M1), and the Pi decodes video above 1080p in software anyway.
# The TV upscales.
OUT="${LUMEN_OUTPUT:-HDMI-A-1}"
MODE="${LUMEN_MODE:-1920x1080@60}"
wlr-randr --output "$OUT" --mode "$MODE" || wlr-randr --output "$OUT" --mode "${MODE%@*}" \
  || echo "start-lumen: could not set $MODE on $OUT, staying at the current mode"
wlr-randr | grep -E "^[^ ]|current" # log what we ended up with
exec "$HOME/lumen-tv/target/release/lumen-tv"
