#!/bin/sh
# Lumen install helper for a Raspberry Pi 5 on Raspberry Pi OS (64-bit).
#
#   sh session/install.sh                       dry run: prints every command, changes nothing
#   sudo sh session/install.sh --apply          do the setup steps (packages, groups, udev, unit, sudoers)
#   ... --apply --set-default                   also make Lumen the boot session (disable kodi.service, enable lumen.service)
#   ... --apply --revert                        make Kodi the boot session again (disable lumen.service, enable kodi.service)
#   ... --apply --quiet-boot                    add quiet kernel options to /boot/firmware/cmdline.txt (backup first)
#
# Nothing here starts or stops a unit; boot changes take effect at the next reboot.
# Kodi's own files are never edited. Safe to run repeatedly.
set -eu

APPLY=0 SET_DEFAULT=0 REVERT=0 QUIET=0
for a in "$@"; do
  case "$a" in
    --apply) APPLY=1 ;;
    --set-default) SET_DEFAULT=1 ;;
    --revert) REVERT=1 ;;
    --quiet-boot) QUIET=1 ;;
    -h|--help) sed -n '2,11p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "install.sh: unknown option $a (see --help)" >&2; exit 2 ;;
  esac
done
if [ "$SET_DEFAULT" = 1 ] && [ "$REVERT" = 1 ]; then
  echo "install.sh: --set-default and --revert exclude each other" >&2; exit 2
fi
if [ "$APPLY" = 0 ] && [ $((SET_DEFAULT + REVERT + QUIET)) -gt 0 ]; then
  echo "install.sh: dry run: --set-default/--revert/--quiet-boot only take effect with --apply; showing what they would do"
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
trap 'rm -f "$UNIT_TMP" "${SUDOERS_TMP:-}" "${CMDLINE_TMP:-}"' EXIT
sed -e "s|/home/laith|$TARGET_HOME|g" -e "s|^User=laith\$|User=$TARGET_USER|" -e "s|^Group=laith\$|Group=$TARGET_GROUP|" "$UNIT_SRC" > "$UNIT_TMP"
if [ -f "$UNIT_DST" ] && cmp -s "$UNIT_TMP" "$UNIT_DST"; then
  say "already installed and identical"
else
  run install -m 0644 "$UNIT_TMP" "$UNIT_DST"
  run systemctl daemon-reload
fi

# --- 5. sudoers ---------------------------------------------------------------------
step "5. /etc/sudoers.d/lumen (switch Kodi <-> Lumen test session without a password)"
SUDOERS_DST=/etc/sudoers.d/lumen
if [ -e "$SUDOERS_DST" ]; then
  say "already installed, left alone"
else
  SUDOERS_TMP=$(mktemp)
  sed "s|^laith |$TARGET_USER |" "$SESSION_DIR/sudoers-lumen" > "$SUDOERS_TMP"
  run visudo -cf "$SUDOERS_TMP"
  run install -m 0440 -o root -g root "$SUDOERS_TMP" "$SUDOERS_DST"
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
  run systemctl disable kodi.service
  run systemctl enable lumen.service
elif [ "$REVERT" = 1 ]; then
  step "7. boot into Kodi again (takes effect at the next reboot)"
  run systemctl disable lumen.service
  run systemctl enable kodi.service
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
  say "Boot default: lumen.service (next reboot). To go back to Kodi:"
  say "  sudo sh $SESSION_DIR/install.sh --apply --revert"
else
  say "Boot default unchanged. lumen.service is installed but not enabled; Kodi still starts at boot."
fi
