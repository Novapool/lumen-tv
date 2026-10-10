#!/bin/sh
# Run by labwc (-S) inside the Lumen session. Sets the TV mode first, 1080p60
# by default (the TV upscales; the Pi decodes video above 1080p in software
# anyway). 4K60 also holds 60 fps now that gradients and shadows are baked
# into images (src/bake.rs): set LUMEN_MODE=3840x2160@60 to use it.
# LUMEN_BIN runs another build (e.g. an experiment built in a separate folder).
OUT="${LUMEN_OUTPUT:-HDMI-A-1}"
MODE="${LUMEN_MODE:-1920x1080@60}"
# The output may not be there yet when labwc starts Lumen (seen on the TV: the
# mode stayed unset, labwc logged "invalid output set for view" and Lumen's
# window came up tiny in the corner). Wait up to ~5 s for it to be listed.
n=0
while [ "$n" -lt 33 ] && ! wlr-randr 2>/dev/null | grep -q "^$OUT "; do
  n=$((n + 1))
  sleep 0.15
done
[ "$n" -gt 0 ] && echo "start-lumen: waited $n x 150 ms for $OUT"
wlr-randr --output "$OUT" --mode "$MODE" || wlr-randr --output "$OUT" --mode "${MODE%@*}" \
  || echo "start-lumen: could not set $MODE on $OUT, staying at the current mode"
wlr-randr | grep -E "^[^ ]|current" # log what we ended up with
exec "${LUMEN_BIN:-$HOME/lumen-tv/target/release/lumen-tv}"
