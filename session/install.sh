#!/bin/sh
# Lumen install helper for a Raspberry Pi 5 on Raspberry Pi OS (64-bit).
#
#   sh session/install.sh                       dry run: prints every command, changes nothing
#   sudo sh session/install.sh --apply          do the setup steps (packages, groups, udev, unit, sudoers)
#   ... --apply --set-default                   also make Lumen the boot session (disable kodi.service if present, enable lumen.service)
#   ... --apply --revert                        stop booting Lumen: console boot (disable lumen.service, keep getty@tty1 enabled)
#   ... --apply --quiet-boot                    add quiet kernel options to /boot/firmware/cmdline.txt (backup first)
#   ... --apply --helper                        install the root install helper (needs target/release/lumen-helper, build it with scripts/pi-build.sh)
#
# Nothing here starts or stops a unit, except --helper enabling lumen-helper.socket.
# Boot changes take effect at the next reboot. Kodi's own files are never edited.
# Safe to run repeatedly. To remove the helper by hand: systemctl disable --now lumen-helper.socket;
# rm /etc/systemd/system/lumen-helper.* /usr/local/libexec/lumen-helper; rm -r /etc/lumen-tv /var/lib/lumen-helper
set -eu

APPLY=0 SET_DEFAULT=0 REVERT=0 QUIET=0 HELPER=0
for a in "$@"; do
  case "$a" in
    --apply) APPLY=1 ;;
    --set-default) SET_DEFAULT=1 ;;
    --revert) REVERT=1 ;;
    --quiet-boot) QUIET=1 ;;
    --helper) HELPER=1 ;;
    -h|--help) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install.sh: unknown option $a (see --help)" >&2; exit 2 ;;
  esac
done
if [ "$SET_DEFAULT" = 1 ] && [ "$REVERT" = 1 ]; then
  echo "install.sh: --set-default and --revert exclude each other" >&2; exit 2
fi
if [ "$APPLY" = 0 ] && [ $((SET_DEFAULT + REVERT + QUIET + HELPER)) -gt 0 ]; then
  echo "install.sh: dry run: --set-default/--revert/--quiet-boot/--helper only take effect with --apply; showing what they would do"
fi

SESSION_DIR=$(cd "$(dirname "$0")" && pwd)
CMDLINE=/boot/firmware/cmdline.txt
QUIET_OPTS="quiet loglevel=3 logo.nologo vt.global_cursor_default=0"

# --- packages (apt names as on Raspberry Pi OS Trixie) -----------------------
# Runtime: the labwc session and the things Lumen calls.
PKG_SESSION="labwc wlr-randr xwayland"          # compositor, output mode switch, X11 apps (Steam Link)
PKG_APPS="chromium flatpak"                      # web apps in kiosk mode, Store flatpak installs
PKG_CEC="v4l-utils"                              # cec-ctl, TV remote claim/re-claim
PKG_BT="bluez"                                   # Bluetooth panel (controllers)
PKG_AUDIO="pipewire pipewire-pulse pipewire-alsa wireplumber"  # sound for apps
PKG_AIRPLAY="uxplay avahi-daemon gstreamer1.0-plugins-bad gstreamer1.0-plugins-good gstreamer1.0-libav python3-gi gir1.2-gtk-3.0"  # AirPlay mirroring (plugins-good has pulsesink)
# Build (cargo build on the Pi; rustup is installed separately, see end of run).
PKG_BUILD="build-essential pkg-config rsync curl"            # toolchain bits, scripts/pi-build.sh uses rsync
PKG_DEV="libssl-dev libfontconfig-dev libfreetype-dev libxkbcommon-dev libwayland-dev libegl-dev libgles-dev libgl-dev libx11-dev libxcb1-dev"  # Slint + femtovg + winit
PACKAGES="$PKG_SESSION $PKG_APPS $PKG_CEC $PKG_BT $PKG_AUDIO $PKG_AIRPLAY $PKG_BUILD $PKG_DEV"

# --- helpers -----------------------------------------------------------------
say() { printf '%s\n' "$*"; }
step() { printf '\n== %s\n' "$*"; }
# run CMD...: print it; execute it only with --apply
run() {
  printf '+ %s\n' "$*"
  if [ "$APPLY" = 1 ]; then "$@"; fi
}
die() { echo "install.sh: $*" >&2; exit 1; }

# --- checks ------------------------------------------------------------------
step "Checking this machine"
[ "$(uname -m)" = "aarch64" ] || die "needs a 64-bit ARM Pi (uname -m is $(uname -m), expected aarch64); nothing changed"
[ -f /etc/os-release ] && grep -q '^ID=\(debian\|raspbian\)$' /etc/os-release || die "needs Raspberry Pi OS / Debian; nothing changed"
[ -f /etc/rpi-issue ] || die "/etc/rpi-issue missing: this does not look like Raspberry Pi OS; nothing changed"
command -v systemctl >/dev/null || die "systemd not found"
if [ "$APPLY" = 1 ] && [ "$(id -u)" -ne 0 ]; then
  die "--apply needs root: run it as 'sudo sh session/install.sh --apply ...'"
fi
TARGET_USER=${SUDO_USER:-$(id -un)}
[ "$TARGET_USER" != root ] || die "run as your normal user via sudo (SUDO_USER is empty), not as a root login"
printf '%s\n' "$TARGET_USER" | grep -Eq '^[a-z_][a-z0-9_-]*$' || die "unexpected user name '$TARGET_USER'"
TARGET_HOME=$(getent passwd "$TARGET_USER" | cut -d: -f6)
TARGET_GROUP=$(id -gn "$TARGET_USER")
[ -n "$TARGET_HOME" ] && [ -d "$TARGET_HOME" ] || die "no home directory for $TARGET_USER"
printf '%s\n' "$TARGET_HOME" | grep -Eq '^/[A-Za-z0-9._/-]+$' || die "unexpected home path '$TARGET_HOME'"
say "ok: aarch64, Raspberry Pi OS, user=$TARGET_USER home=$TARGET_HOME $([ "$APPLY" = 1 ] && echo '(APPLY)' || echo '(dry run)')"

# --- helper preflight: refuse before any step runs ---------------------------------------
if [ "$HELPER" = 1 ]; then
  HELPER_BIN=$TARGET_HOME/lumen-tv/target/release/lumen-helper
  HELPER_DST=/usr/local/libexec/lumen-helper
  ALLOW_SRC=$SESSION_DIR/helper-allowlist.toml
  ALLOW_DST=/etc/lumen-tv/helper-allowlist.toml
  [ ! -L "$HELPER_BIN" ] || die "$HELPER_BIN is a symlink: refusing (it becomes root code); nothing changed"
  [ -f "$HELPER_BIN" ] || die "$HELPER_BIN not found or not a regular file: run scripts/pi-build.sh from the Mac first; nothing changed"
  [ -f "$ALLOW_SRC" ] || die "$ALLOW_SRC not found; nothing changed"
  for f in lumen-helper.socket lumen-helper.service; do
    [ -f "$SESSION_DIR/$f" ] || die "$SESSION_DIR/$f not found; nothing changed"
  done
fi

# --- 1. packages ---------------------------------------------------------------
step "1. apt packages"
run apt-get update
run apt-get install -y --no-install-recommends $PACKAGES

# --- 2. groups -----------------------------------------------------------------
step "2. $TARGET_USER in input, video, render, audio"
for g in input video render audio; do
  if id -nG "$TARGET_USER" | tr ' ' '\n' | grep -qx "$g"; then
    say "already in $g"
  else
    run usermod -aG "$g" "$TARGET_USER"
  fi
done

# --- 3. /dev/uinput for the input group -----------------------------------------
# Lumen's controller-to-keys feature writes /dev/uinput. On pitv it is already
# root:input 0660 (Raspberry Pi OS ships a udev rule for that); only add a rule
# when the node is not group-writable by input.
step "3. /dev/uinput access"
UDEV_RULE=/etc/udev/rules.d/70-lumen-uinput.rules
if [ -e /dev/uinput ] && [ "$(stat -c %G /dev/uinput)" = input ] && [ "$(( 0$(stat -c %a /dev/uinput) & 060 ))" -eq 48 ]; then
  say "already root:input and group-writable, no rule needed"
elif [ -f "$UDEV_RULE" ]; then
  say "$UDEV_RULE already present"
else
  say "+ write $UDEV_RULE: KERNEL==\"uinput\", GROUP=\"input\", MODE=\"0660\""
  say "+ write /etc/modules-load.d/lumen-uinput.conf: uinput"
  if [ "$APPLY" = 1 ]; then
    printf 'KERNEL=="uinput", SUBSYSTEM=="misc", GROUP="input", MODE="0660"\n' > "$UDEV_RULE"
    printf 'uinput\n' > /etc/modules-load.d/lumen-uinput.conf
    udevadm control --reload-rules
    modprobe uinput || true
    udevadm trigger --name-match=uinput || true
  fi
fi

# --- 4. systemd unit -------------------------------------------------------------
step "4. /etc/systemd/system/lumen.service (user $TARGET_USER)"
UNIT_SRC=$SESSION_DIR/lumen.service
UNIT_DST=/etc/systemd/system/lumen.service
[ -f "$UNIT_SRC" ] || die "$UNIT_SRC not found"
UNIT_TMP=$(mktemp)
TEST_TMP=$(mktemp)
trap 'rm -f "$UNIT_TMP" "$TEST_TMP" "${SUDOERS_TMP:-}" "${CMDLINE_TMP:-}"' EXIT
sed -e "s|/home/laith|$TARGET_HOME|g" -e "s|^User=laith\$|User=$TARGET_USER|" -e "s|^Group=laith\$|Group=$TARGET_GROUP|" "$UNIT_SRC" > "$UNIT_TMP"
RELOAD=0
if [ -f "$UNIT_DST" ] && cmp -s "$UNIT_TMP" "$UNIT_DST"; then
  say "already installed and identical"
else
  run install -m 0644 "$UNIT_TMP" "$UNIT_DST"
  RELOAD=1
fi

# --- 4b. test-session unit -------------------------------------------------------------
step "4b. /etc/systemd/system/lumen-test.service (user $TARGET_USER)"
TEST_SRC=$SESSION_DIR/lumen-test.service
TEST_DST=/etc/systemd/system/lumen-test.service
[ -f "$TEST_SRC" ] || die "$TEST_SRC not found"
sed -e "s|/home/laith|$TARGET_HOME|g" -e "s|^User=laith\$|User=$TARGET_USER|" -e "s|^Group=laith\$|Group=$TARGET_GROUP|" "$TEST_SRC" > "$TEST_TMP"
if [ -f "$TEST_DST" ] && cmp -s "$TEST_TMP" "$TEST_DST"; then
  say "already installed and identical"
else
  run install -m 0644 "$TEST_TMP" "$TEST_DST"
  RELOAD=1
fi
[ "$RELOAD" = 0 ] || run systemctl daemon-reload

# --- 5. sudoers ---------------------------------------------------------------------
step "5. /etc/sudoers.d/lumen (start/stop the Lumen test session without a password)"
SUDOERS_DST=/etc/sudoers.d/lumen
if [ -e "$SUDOERS_DST" ]; then
  say "already installed, left alone"
else
  SUDOERS_TMP=$(mktemp)
  sed "s|^laith |$TARGET_USER |" "$SESSION_DIR/sudoers-lumen" > "$SUDOERS_TMP"
  run visudo -cf "$SUDOERS_TMP"
  run install -m 0440 -o root -g root "$SUDOERS_TMP" "$SUDOERS_DST"
fi

# --- 5b. root install helper (optional) ----------------------------------------------------
if [ "$HELPER" = 1 ]; then
  step "5b. root install helper (--helper)"
  # group that may connect to the socket
  if getent group lumen-helper >/dev/null; then
    say "group lumen-helper exists"
  else
    run groupadd --system lumen-helper
  fi
  if id -nG "$TARGET_USER" | tr ' ' '\n' | grep -qx lumen-helper; then
    say "$TARGET_USER already in lumen-helper"
  else
    run usermod -aG lumen-helper "$TARGET_USER"
    NEW_GROUP=1
  fi

  # root-owned copy of the binary, never the user-writable build output
  say "This binary becomes root code (run by systemd as root):"
  say "  sha256 $(sha256sum "$HELPER_BIN" | cut -d' ' -f1)  $HELPER_BIN"
  run install -d -o root -g root -m 0755 /usr/local/libexec
  run install -o root -g root -m 0755 "$HELPER_BIN" "$HELPER_DST"

  # allowlist: root-owned; show a diff before replacing an existing one
  run install -d -o root -g root -m 0755 /etc/lumen-tv
  if [ -f "$ALLOW_DST" ] && ! cmp -s "$ALLOW_SRC" "$ALLOW_DST"; then
    say "allowlist differs from the installed one:"
    diff -u "$ALLOW_DST" "$ALLOW_SRC" || true
  fi
  if [ -f "$ALLOW_DST" ] && cmp -s "$ALLOW_SRC" "$ALLOW_DST"; then
    say "allowlist already installed and identical"
  else
    run install -o root -g root -m 0644 "$ALLOW_SRC" "$ALLOW_DST"
  fi

  # units: only the socket is enabled
  HRELOAD=0 SOCKET_CHANGED=0
  for f in lumen-helper.socket lumen-helper.service; do
    if [ -f "/etc/systemd/system/$f" ] && cmp -s "$SESSION_DIR/$f" "/etc/systemd/system/$f"; then
      say "$f already installed and identical"
    else
      run install -o root -g root -m 0644 "$SESSION_DIR/$f" "/etc/systemd/system/$f"
      HRELOAD=1
      [ "$f" != lumen-helper.socket ] || SOCKET_CHANGED=1
    fi
  done
  [ "$HRELOAD" = 0 ] || run systemctl daemon-reload
  run systemctl enable --now lumen-helper.socket
  if [ "$SOCKET_CHANGED" = 1 ] && systemctl is-active --quiet lumen-helper.socket; then
    say "socket unit changed and the socket is active: restarting it to pick up the change"
    run systemctl restart lumen-helper.socket
  elif [ "$SOCKET_CHANGED" = 1 ]; then
    say "socket unit changed; it is not active yet (enable --now starts it with the new unit)"
  fi

  if [ "$APPLY" = 1 ]; then
    "$HELPER_DST" check || say "WARNING: lumen-helper check reported a problem (see above)"
  else
    say "+ $HELPER_DST check"
  fi
fi

# --- 6. quiet boot (optional) --------------------------------------------------------
if [ "$QUIET" = 1 ]; then
  step "6. quiet boot options in $CMDLINE"
  [ -f "$CMDLINE" ] || die "$CMDLINE not found"
  missing=""
  for o in $QUIET_OPTS; do
    tr ' ' '\n' < "$CMDLINE" | grep -qx -- "$o" || missing="$missing $o"
  done
  if [ -z "$missing" ]; then
    say "all quiet options already present"
  else
    say "adding:$missing"
    BACKUP=$CMDLINE.lumen-backup
    run cp -n "$CMDLINE" "$BACKUP"
    say "+ write $CMDLINE as one line: existing line +$missing"
    if [ "$APPLY" = 1 ]; then
      CMDLINE_TMP=$(mktemp "$(dirname "$CMDLINE")/.cmdline.XXXXXX")  # same filesystem, so mv is atomic
      printf '%s%s\n' "$(tr -d '\n' < "$CMDLINE" | sed 's/[[:space:]]*$//')" "$missing" > "$CMDLINE_TMP"
      [ "$(wc -l < "$CMDLINE_TMP")" -eq 1 ] || { rm -f "$CMDLINE_TMP"; die "cmdline would not be a single line; left untouched"; }
      chmod "$(stat -c %a "$CMDLINE")" "$CMDLINE_TMP"
      mv -f "$CMDLINE_TMP" "$CMDLINE"
    fi
  fi
fi

# --- 7. boot switch (only with the flags) ----------------------------------------------
if [ "$SET_DEFAULT" = 1 ]; then
  step "7. boot into Lumen (takes effect at the next reboot)"
  if systemctl cat kodi.service >/dev/null 2>&1; then
    run systemctl disable kodi.service
  else
    say "kodi.service not present"
  fi
  run systemctl enable lumen.service
elif [ "$REVERT" = 1 ]; then
  step "7. console boot (takes effect at the next reboot)"
  run systemctl disable lumen.service
  if systemctl is-enabled --quiet getty@tty1.service 2>/dev/null; then
    say "getty@tty1 already enabled"
  else
    run systemctl enable getty@tty1.service
  fi
fi

# --- summary ---------------------------------------------------------------------------
step "Summary"
if [ "$APPLY" = 0 ]; then
  say "Dry run: nothing was changed. Add --apply (as root) to do the steps above."
else
  say "Done. Group changes need a new login session (a reboot covers it)."
fi
command -v cargo >/dev/null 2>&1 || [ -x "$TARGET_HOME/.cargo/bin/cargo" ] \
  || say "Rust is not installed for $TARGET_USER: install it with rustup (https://rustup.rs) before building."
if [ "$SET_DEFAULT" = 1 ]; then
  say "Boot default: lumen.service (next reboot). To stop booting Lumen (console login on tty1):"
  say "  sudo sh $SESSION_DIR/install.sh --apply --revert"
elif [ "$REVERT" = 1 ]; then
  say "Console boot (next reboot): lumen.service disabled, getty@tty1 enabled, no autologin."
else
  say "Boot default unchanged (lumen.service is enabled or not, as before)."
fi
if [ "$HELPER" = 1 ]; then
  say "Set the PIN: sudo /usr/local/libexec/lumen-helper set-pin"
  if [ "${NEW_GROUP:-0}" = 1 ]; then
    say "$TARGET_USER was just added to lumen-helper: run 'sudo systemctl restart lumen.service' or reboot"
    say "(Power -> Restart Lumen keeps the old groups)."
  fi
fi
