#!/bin/sh
# ExecStart of lumen-slint-test.service. Default: labwc with the spike as its
# only client. LUMEN_KMS=1: the spike straight on tty1 (Slint linuxkms backend).
D="$HOME/lumen-slint"
if [ "$LUMEN_KMS" = 1 ]; then
  export SLINT_BACKEND="${SLINT_BACKEND:-linuxkms-femtovg}"
  exec "$D/target/release/lumen-slint"
fi
exec /usr/bin/labwc -C "$D/session/labwc" -S "$D/session/start.sh"
