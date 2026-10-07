#!/bin/sh
# Run by labwc (-S): set the TV mode, then run the spike. Same as
# lumen-tv/session/start-lumen.sh apart from the binary.
OUT="${LUMEN_OUTPUT:-HDMI-A-1}"
MODE="${LUMEN_MODE:-1920x1080@60}"
wlr-randr --output "$OUT" --mode "$MODE" || wlr-randr --output "$OUT" --mode "${MODE%@*}" \
  || echo "start: could not set $MODE on $OUT, staying at the current mode"
wlr-randr | grep -E "^[^ ]|current"
exec "$HOME/lumen-slint/target/release/lumen-slint"
