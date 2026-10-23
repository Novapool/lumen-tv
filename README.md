# Lumen

Lumen is an Apple TV-style launcher for a Raspberry Pi 5 connected to a TV. It is a Rust + Slint app that runs as the only client of a labwc (Wayland) session, takes input from a TV remote (HDMI-CEC), keyboard or game controller, and launches apps (web apps in a kiosk browser, or any command). It is meant to replace Kodi as the box's boot screen. GPLv3, see `LICENSE`.

This README is written for Laith's box (`pitv`). Milestone M6 rewrites it for other people.

## Setup on a Pi 5

Needs Raspberry Pi OS (64-bit, Debian Trixie). `session/install.sh` refuses to run anywhere else.

Everything is a **dry run by default**: it prints each command and changes nothing.

```sh
sh session/install.sh                                  # dry run: print the plan
sudo sh session/install.sh --apply                     # do it
sudo sh session/install.sh --apply --set-default       # also boot into Lumen instead of Kodi
sudo sh session/install.sh --apply --quiet-boot        # also quiet the boot text
sudo sh session/install.sh --apply --revert            # boot into Kodi again
```

`--apply` installs the apt packages (labwc, wlr-randr, xwayland, chromium, flatpak, v4l-utils, bluez, PipeWire, build tools and dev libraries), adds your user to `input video render audio`, adds a `/dev/uinput` udev rule only if the node isn't already group-writable by `input`, installs `/etc/systemd/system/lumen.service` (user and home substituted), and installs `/etc/sudoers.d/lumen` if missing (an existing `/etc/sudoers.d/lumen` is never updated by `install.sh`; copy `session/sudoers-lumen` over it yourself if it changed). Rust is not installed for you: use [rustup](https://rustup.rs).

Flags:
- `--set-default`: disables `kodi.service`, enables `lumen.service`. Takes effect at the next reboot.
- `--revert`: the reverse (disables `lumen.service`, enables `kodi.service`). Excludes `--set-default`.
- `--quiet-boot`: appends `quiet loglevel=3 logo.nologo vt.global_cursor_default=0` to `/boot/firmware/cmdline.txt` (backup at `cmdline.txt.lumen-backup`, kept as a single line).
- Without `--apply`, the three flags only show what they would do.

The script never starts or stops a unit and never edits Kodi's files. It is safe to run repeatedly. Group changes need a new login (a reboot covers it).

### Boot unit behaviour

`lumen.service` runs `labwc` with `session/start-lumen.sh` on tty1, `Restart=always`: any exit restarts the session (a clean exit would otherwise leave the TV black). An explicit `systemctl stop`, or a conflicting unit starting (`steamlink`, `uxplay`, `kodi`, `lumen-test`), is not restarted. `StartLimitBurst=3` in 60 s: the 3rd exit within 60 s makes the unit fail and `OnFailure=kodi.service` hands the TV to Kodi (rescue). After a rescue, a plain `systemctl start lumen.service` is refused for a while (start limit); bring Lumen back with `sudo systemctl reset-failed lumen.service && sudo systemctl start lumen.service`. `start-lumen.sh` waits for the HDMI output, sets the mode, then `exec`s the binary.

## Building

On the Pi: `cargo build --release` in this folder (binary: `target/release/lumen-tv`, which is what `start-lumen.sh` runs).

From the Mac: `scripts/pi-build.sh` rsyncs the repo to `pitv:~/lumen-tv/` and runs `cargo build --release` there. It restarts nothing.

## Testing without changing the boot default

`session/lumen-test.service` is a manual session on tty1 that `Conflicts=kodi.service`. When Lumen exits, Kodi is started again (`ExecStopPost`), so the TV never stays black. `session/sudoers-lumen` lets the user switch without a password:

```sh
sudo systemctl start --no-block lumen-test.service   # Kodi stops, Lumen starts
sudo systemctl stop lumen-test.service               # back to Kodi
```

## Adding an app by hand

One TOML file per app in `~/.config/lumen-tv/apps/`. The three built-in apps (Jellyfin, Steam Link, Kodi) are written there on first start only; edit them freely. A broken file is skipped and listed under Settings, never fatal. Apps load at start and after a Store install or uninstall; restart Lumen after hand edits. Unknown keys are an error.

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

Example, `~/.config/lumen-tv/apps/youtube.toml`:

```toml
id = "youtube"
name = "YouTube"
type = "url"
url = "https://www.youtube.com/tv"
background = "keep-alive"
app_id = "lumen-youtube"
glyph = "Y"
colors = ["#7a0b0b", "#ff3030"]
order = 20
```

URL apps open with `$LUMEN_BROWSER` or the default `chromium --kiosk --ozone-platform=wayland ... --class=lumen-{id} ... {url}` (`{id}` and `{url}` are substituted; without `{url}` the URL is appended).

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
| `LUMEN_INPUT_DEBUG` | `1` starts with raw key-name recording on. |

## License

GPLv3, see `LICENSE`.
