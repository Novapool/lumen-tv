#!/bin/sh
# Run by labwc (-S) inside the Lumen session. Sets the TV mode first, 1080p60
# by default (the TV upscales; the Pi decodes video above 1080p in software
# anyway). 4K60 also holds 60 fps now that gradients and shadows are baked
# into images (src/bake.rs).
# Mode order: `mode = "WIDTHxHEIGHT@RATE"` in settings.toml (written by the
# Settings resolution switch), else LUMEN_MODE, else 1920x1080@60.
# LUMEN_BIN runs another build (e.g. an experiment built in a separate folder).
OUT="${LUMEN_OUTPUT:-HDMI-A-1}"
DEFAULT_MODE="1920x1080@60"
SETTINGS="${XDG_CONFIG_HOME:-$HOME/.config}/lumen-tv/settings.toml"

valid_mode() { # WIDTHxHEIGHT@RATE, digits only (RATE may have a decimal part)
  printf '%s\n' "$1" | grep -Eq '^[0-9]{3,5}x[0-9]{3,5}@[0-9]{1,3}(\.[0-9]{1,3})?$'
}

MODE=""
if [ -r "$SETTINGS" ]; then
  # Top-level `mode` key only (stop at the first [table] header); any TOML
  # spacing around `=`; optional trailing comment. Value is validated below.
  cand=$(sed -n '/^[[:space:]]*\[/q; s/^[[:space:]]*mode[[:space:]]*=[[:space:]]*"\([^"]*\)"[[:space:]]*\(#.*\)\{0,1\}$/\1/p' "$SETTINGS" | head -n 1)
  if [ -n "$cand" ]; then
    if valid_mode "$cand"; then MODE="$cand"; echo "start-lumen: mode $MODE from settings.toml"
    else echo "start-lumen: ignoring invalid mode in settings.toml"; fi
  fi
fi
if [ -z "$MODE" ] && [ -n "${LUMEN_MODE:-}" ]; then
  if valid_mode "$LUMEN_MODE"; then MODE="$LUMEN_MODE"
  else echo "start-lumen: ignoring invalid LUMEN_MODE"; fi
fi
[ -n "$MODE" ] || MODE="$DEFAULT_MODE"

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
