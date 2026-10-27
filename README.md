# Lumen

Lumen is an Apple TV-style launcher for a Raspberry Pi 5 connected to a TV. It is a Rust + Slint app that runs as the only client of a labwc (Wayland) session, takes input from a TV remote (HDMI-CEC), keyboard or game controller, and launches apps (web apps in a kiosk browser, or any command). It replaces Kodi as the box's boot screen (Kodi is now an optional Store app). GPLv3, see `LICENSE`.

This README is written for Laith's box (`pitv`). Milestone M6 rewrites it for other people.

## Setup on a Pi 5

Needs Raspberry Pi OS (64-bit, Debian Trixie). `session/install.sh` refuses to run anywhere else.

Everything is a **dry run by default**: it prints each command and changes nothing.

```sh
sh session/install.sh                                  # dry run: print the plan
sudo sh session/install.sh --apply                     # do it
sudo sh session/install.sh --apply --set-default       # also boot into Lumen
sudo sh session/install.sh --apply --quiet-boot        # also quiet the boot text
sudo sh session/install.sh --apply --revert            # stop booting Lumen (console login on tty1)
sudo sh session/install.sh --apply --helper            # also install the root install helper (see below)
```

`--apply` installs the apt packages (labwc, wlr-randr, xwayland, chromium, flatpak, v4l-utils, bluez, PipeWire, uxplay, avahi-daemon, GStreamer bad/libav plugins, python3-gi + GTK 3 (AirPlay tile), build tools and dev libraries), adds your user to `input video render audio`, adds a `/dev/uinput` udev rule only if the node isn't already group-writable by `input`, installs `/etc/systemd/system/lumen.service` (user and home substituted), and installs `/etc/sudoers.d/lumen` if missing (an existing `/etc/sudoers.d/lumen` is never updated by `install.sh`; copy `session/sudoers-lumen` over it yourself if it changed). Rust is not installed for you: use [rustup](https://rustup.rs).

Flags:
- `--set-default`: enables `lumen.service` (and disables `kodi.service` if that old unit is still present). Takes effect at the next reboot.
- `--revert`: stop booting Lumen: disables `lumen.service` and makes sure `getty@tty1` is enabled (a plain console login, no autologin). Nothing enables Kodi. Excludes `--set-default`.
- `--quiet-boot`: appends `quiet loglevel=3 logo.nologo vt.global_cursor_default=0` to `/boot/firmware/cmdline.txt` (backup at `cmdline.txt.lumen-backup`, kept as a single line).
- `--helper`: installs the root install helper (see "Root installs (PIN helper)"). Needs `target/release/lumen-helper` built first (`scripts/pi-build.sh`). The only flag that starts a unit (`lumen-helper.socket`).
- Without `--apply`, the flags only show what they would do.

The script never starts or stops a unit and never edits Kodi's files (or any Kodi-era unit). It is safe to run repeatedly. Group changes need a new login (a reboot covers it).

### Root installs (PIN helper)

Store apps that need apt (`root: true`) go through `lumen-helper`, a small root daemon. Lumen itself never runs as root and has no sudo rights for this.

- **What runs as root:** `/usr/local/libexec/lumen-helper serve`, started on demand by `lumen-helper.socket` (`/run/lumen-helper.sock`, root:`lumen-helper` 0660) and exiting after 60 s idle. Nothing root runs while idle.
- **What it trusts:** only members of the `lumen-helper` group can connect (`--helper` adds your user), and every install/remove needs the PIN. Package names come only from the root-owned allowlist `/etc/lumen-tv/helper-allowlist.toml`; the Store manifest supplies just an app id, used as a lookup key. The helper refuses to run if the allowlist, `/etc/lumen-tv` or `/etc` is writable by anyone but root. apt is run as an argument vector (never a shell) with a cleared environment, after a `-s` simulation: a remove that would install anything, or remove a package not in the entry's `remove` list, is refused.
- **What it doesn't do:** change the PIN from the TV, purge packages, or run anything for an id that isn't in the allowlist. Wrong PINs lock it: 5 in a row lock for 60 s, doubling up to 1 h; the counter survives restarts.

Install (build first with `scripts/pi-build.sh` from the Mac):

```sh
sh session/install.sh --helper             # dry run: prints every command
sudo sh session/install.sh --apply --helper
```

This creates the `lumen-helper` group and adds your user, installs the binary to `/usr/local/libexec/lumen-helper` (a root-owned copy, not the build output), installs the allowlist and the `lumen-helper.socket`/`.service` units, enables only the socket, and runs `lumen-helper check`. It refuses (before any step) if the build output is missing or a symlink, prints the binary's sha256 (this file becomes root code), copies units only when they differ, and restarts an active socket if its unit changed. If your user was just added to the group, reboot or `sudo systemctl restart lumen.service` (Power > Restart Lumen keeps the old groups).

**Set the PIN** over SSH (reads twice, no echo; 4-8 digits, 6 suggested). Until it is set, root installs say so:

```sh
sudo /usr/local/libexec/lumen-helper set-pin
```

Run it again to change the PIN. The PIN hash (argon2id) lives in `/var/lib/lumen-helper/`.

**Add an allowlist set:** add a table to `session/helper-allowlist.toml` (the table name is the Store app id; keys `install`, `remove`, optional `update`; unknown keys are rejected), then re-run `sudo sh session/install.sh --apply --helper`. It shows a diff before replacing the installed copy.

**Remove the helper by hand:**

```sh
sudo systemctl disable --now lumen-helper.socket
sudo rm /etc/systemd/system/lumen-helper.socket /etc/systemd/system/lumen-helper.service /usr/local/libexec/lumen-helper
sudo rm -r /etc/lumen-tv /var/lib/lumen-helper
sudo systemctl daemon-reload
```

(`sudo groupdel lumen-helper` too, if you want.) `LUMEN_HELPER_SOCKET` (in the env file) points Lumen at another socket path, for tests. The helper's own `--socket`, `--state-dir`, `--allowlist` and `--apt` flags only work with `--insecure-test-mode`; never use that on the Pi.

### Boot unit behaviour

`lumen.service` runs `labwc` with `session/start-lumen.sh` on tty1, `Restart=always`: any exit restarts the session (a clean exit would otherwise leave the TV black). An explicit `systemctl stop`, or a conflicting unit starting (`lumen-test`; the Kodi-era `steamlink`, `uxplay` and `kodi` units are still named in `Conflicts=` until they're removed), is not restarted. `StartLimitBurst=3` in 60 s: the 3rd exit within 60 s makes the unit fail and `OnFailure=getty@tty1.service` puts a login prompt on the TV (the console rescue; no autologin, SSH still works). After a rescue, a plain `systemctl start lumen.service` is refused for a while (start limit); bring Lumen back with `sudo systemctl reset-failed lumen.service && sudo systemctl start lumen.service`. `start-lumen.sh` waits for the HDMI output, sets the mode, then `exec`s the binary.

### Power

The last tile on Home opens Power: Restart Lumen, Reboot, Shut down, each behind a confirm step (Cancel is focused; Back cancels). Restart Lumen exits, and `Restart=always` brings the session back (under `lumen-test.service` it also comes back: stopping the test session starts `lumen.service`). Reboot and Shut down run `systemctl reboot`/`poweroff`: polkit lets the active local session on tty1 do that without a password, so no sudoers entry is needed. Three Restart Lumen presses within 60 s hit the start limit and leave a tty1 login prompt on the TV.

## Building

On the Pi: `cargo build --release` in this folder (binary: `target/release/lumen-tv`, which is what `start-lumen.sh` runs).

From the Mac: `scripts/pi-build.sh` rsyncs the repo to `pitv:~/lumen-tv/` and runs `cargo build --release` there. It restarts nothing.

## Testing without changing the boot default

`session/lumen-test.service` is a manual session on tty1 that `Conflicts=` with `getty@tty1` and `lumen.service`. When it exits, `lumen.service` is started again (`ExecStopPost`), so the TV never stays black. `session/sudoers-lumen` lets the user switch without a password:

```sh
sudo systemctl start --no-block lumen-test.service   # lumen.service stops, the test session starts
sudo systemctl stop lumen-test.service               # back to the boot session
```

## Adding an app by hand

One TOML file per app in `~/.config/lumen-tv/apps/`. The built-in apps (Jellyfin, YouTube, AirPlay, Steam Link; Kodi is a Store app, not a default) are written there when missing, and each is offered once: `~/.config/lumen-tv/apps/.shipped` lists the ids already offered, so an app you delete or uninstall stays gone, and an existing file is never overwritten. A Lumen update that adds a new built-in app writes just that one. Edit them freely. A broken file is skipped and listed under Settings, never fatal. Apps load at start and after a Store install or uninstall; restart Lumen after hand edits. Unknown keys are an error.

| Field | Meaning |
|---|---|
| `id` | Required. Letters, digits, `-`, `_`. |
| `name` | Required. Tile label. |
| `type` | `"command"` (default) or `"url"`. |
| `launch` | Required for `command`. Run as `sh -c 'exec <launch>'`; `~` expands. |
| `url` | Required for `url`. Opened in the kiosk browser. |
| `fallback_url` | Used when `launch`'s program isn't on PATH. |
| `background` | `"close-on-exit"` (default) or `"keep-alive"` (stays running when you go Home). |
| `exclusive` | `true` stops other apps while it runs. Default false. |
| `icon` | Icon name or path (resolved by `src/icons.rs`). |
| `app_id` | Wayland app_id / X11 class of the app's windows. |
| `process` | Real executable (`~` expanded; a bare name matches by comm), for launchers that exit at once (Steam Link). |
| `glyph` | 1-2 characters shown when there is no icon. |
| `colors` | 2-3 `"#rrggbb"` stops for the tile gradient. |
| `order` | Sort position (lower first). |
| `user_agent` | `url` apps (or a `command` with `fallback_url`). Sent as `--user-agent=<value>`; one line, not empty. |
| `browser_args` | Same apps. Extra Chromium arguments, each starting with `--`. A `~/` right after the first `=` expands to `$HOME`. Keep at most one `--enable-features=` and one `--disable-features=` (Chromium honours only the last). |
| `keys` | A `[keys]` table, e.g. `Back = "KEY_ESC"`: while this app is in front, the TV remote's matching button sends that key instead of the default (any app), and so does the pad's (web apps only). Actions: `Up Down Left Right Select Back PlayPause`; keys are `KEY_*` names. |

A complete browser example with a user agent, extension arguments and a `[keys]` table ships as `config/apps/youtube.toml`.

URL apps open with `$LUMEN_BROWSER` or the default `chromium --kiosk --ozone-platform=wayland ... --class=lumen-{id} ... {url}` (`{id}` and `{url}` are substituted; without `{url}` the URL is appended). The entry's `user_agent` and `browser_args` go where `{args}` is in the template, else just before the URL; with neither set the line is exactly the template.

## Adding a controller mapping

Mapping files turn a device's raw evdev events into Lumen actions: `Up Down Left Right Select Back Home Menu PlayPause`. Lumen ships `keyboard`, `cec-remote`, `gamepad`, `xbox` and `playstation` (in `config/input/`, built in). A file of the same name in `~/.config/lumen-tv/input/` replaces the shipped one; any other `*.toml` there is added. Mappings are read at start, so restart Lumen after changes. Settings has a Mappings panel showing what loaded.

File layout:
- `deadzone`: 0.05-0.95 (default 0.4), how far a stick must move to count as a press.
- `[match]`: at least one of `class` (`"gamepad"` = has BTN_SOUTH, or `"keyboard"`), `ids` (hex vendor `"054c"` or vendor:product `"054c:0ce6"`), `names` (case-insensitive substrings of the device name).
- `[keys]`: `KEY_`/`BTN_` code name = action.
- `[axes]`: `ABS_` code name = `[action below centre, action above centre]`.

Find codes and ids with `evtest` (`sudo apt install evtest`): run `sudo evtest`, pick the device (its name, vendor and product are printed), press buttons and note the `BTN_*`/`ABS_*` names. Settings > Toggles can also record raw key names (input debug).

Example (illustrative only: the vendor id `2dc8` below was written from memory and is NOT confirmed; check the real ids with `evtest` or `lsusb` before using it), a 8BitDo pad that reports as an Xbox-style controller, `~/.config/lumen-tv/input/8bitdo.toml` (based on `config/input/playstation.toml`):

```toml
deadzone = 0.4

[match]
class = "gamepad"
ids = ["2dc8"]
names = ["8BitDo"]

[keys]
BTN_SOUTH = "Select"
BTN_EAST = "Back"
BTN_START = "Menu"
BTN_MODE = "Home"

[axes]
ABS_X = ["Left", "Right"]
ABS_Y = ["Up", "Down"]
ABS_HAT0X = ["Left", "Right"]
ABS_HAT0Y = ["Up", "Down"]
```

## Pointing at a different Store manifest

The Store reads a JSON manifest (`store/manifest.json` in this repo, served from GitHub raw by default). To use another one, either:
- set `store_url` in `~/.config/lumen-tv/settings.toml`, or
- set `LUMEN_STORE_URL` in the env file.

Precedence: `settings.toml` first, then `LUMEN_STORE_URL`, then the built-in URL. A source starting with `/` or `file://` is read from disk; anything else is fetched with `curl`. Check a manifest without the UI:

```sh
./target/release/lumen-tv --store-check                 # the configured source
./target/release/lumen-tv --store-check <url-or-path>   # another one
```

It prints each app with its install/uninstall commands and any errors; exit 0 means no errors. Lumen shows install commands before running them.

## Resolution

Default output mode is 1080p60; 4K60 also holds 60 fps. Change it in Settings > Toggles > Resolution (applied right away and saved), or set `mode` in `~/.config/lumen-tv/settings.toml`:

```toml
mode = "3840x2160@60"
```

Format is `WIDTHxHEIGHT@RATE`. Precedence: `settings.toml` `mode`, then `LUMEN_MODE`, then `1920x1080@60`. `start-lumen.sh` applies it at session start. While running, a watcher re-applies the mode if the TV's power cycle resets the output to its own preferred mode. Settings > Display shows the saved mode and its source. `settings.toml` accepts only `mode` and `store_url`; a bad file gives defaults plus an error in Settings, and Lumen won't overwrite it.

## Env file

`~/.config/lumen-tv/env` (`KEY=value` lines, read by systemd via `EnvironmentFile`; edit without sudo, then restart the session):

| Variable | Effect |
|---|---|
| `LUMEN_MODE` | Output mode, used only if `settings.toml` has no `mode`. |
| `LUMEN_STORE_URL` | Store manifest, used only if `settings.toml` has no `store_url`. |
| `LUMEN_BENCH` | `1` starts the frame-time bench 3 s after launch; `bt` benches the Bluetooth panel. |
| `LUMEN_BIN` | Run another binary instead of `~/lumen-tv/target/release/lumen-tv` (read by `start-lumen.sh`). |
| `LUMEN_BROWSER` | Kiosk browser command template for `url` apps. |
| `LUMEN_BT` | `fake` uses a fake Bluetooth backend (for UI testing). |
| `LUMEN_OUTPUT` | Output name for mode switching (default `HDMI-A-1`). |
| `LUMEN_HELPER_SOCKET` | Root install helper socket (default `/run/lumen-helper.sock`). |
| `LUMEN_INPUT_DEBUG` | `1` starts with raw key-name recording on. |

## License

GPLv3, see `LICENSE`.
