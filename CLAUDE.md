# lumen-tv

Lumen: an Apple TV-style launcher for pitv (Rust + Slint UI, evdev input, labwc session), that replaced Kodi on pitv. GPLv3. Pi access, network and box rules are in `../CLAUDE.md`.

## Docs (read when)
- `README.md`: setup, adding apps/mappings, settings and env vars. Read before changing any of those.
- `BRIEF.md`: Laith's original spec. Read before planning or changing scope. Never edit it (scope changes go in MILESTONES.md).
- `MILESTONES.md`: scope changes, milestone specs + Gate lines. Read before starting or closing a milestone.
- `IN-PROGRESS.md`: current state, Slint rules, open questions, next steps. Read before any work.
- `docs/ARCHIVE.md`: old milestone execution notes. Read on demand only.

## Layout
- `src/`: Rust (main + Slint callbacks, input, registry, apps, icons, bake, `diag.rs`/`logtail.rs` = Settings panel data + log capture, `reclaim.rs` = CEC remote re-claim, `home.rs` = Home tiles + live reload, `store.rs`/`storefeed.rs` = Store core + Slint wiring, `bluetooth.rs`/`btfeed.rs` = Bluetooth panel core + Slint wiring, `input/keys_out.rs` = controller → keys for web apps, `session.rs` = glue between registry, lifecycle, windows and the Home gate, `settings.rs` = settings.toml (mode, store_url), `power.rs` = Power tile actions, `platform/` incl. `output.rs` = HDMI mode watcher). `ui/`: `.slint` files (`settings.slint` = Settings, `store.slint`/`details.slint` = Store, `bluetooth.slint` = Bluetooth panel, `power.slint` = Power screen), compiled by `build.rs`.
- `config/input/`: shipped input mapping files (built into the binary). `config/apps/`: default app registry TOMLs (built in; seeded to `~/.config/lumen-tv/apps/`, new ones once via `apps/.shipped`). `config/browser/youtube-tv/`: Chromium extension for the YouTube tile (no AV1, ≤ 1080p).
- `examples/`: `fake-pad.rs` (virtual controller; `dualsense` mode), `press.rs` (taps keys via uinput) and `toplevels.rs` (lists labwc toplevels): the M3 gate instruments.
- `helper/`: workspace crate `lumen-helper`, the root install daemon (socket-activated, PIN + allowlist). Client side: `src/platform/installer.rs`; PIN pad: `ui/pinpad.slint`.
- `store/manifest.json`: the Store's app list, served from GitHub raw.
- `session/`: labwc config, `start-lumen.sh`, `lumen-airplay` (AirPlay tile: UxPlay in labwc, tests in `test_lumen_airplay.py`), `lumen.service` (boot unit), `lumen-test.service`, `install.sh` (dry run by default; `--helper` installs the root helper), sudoers, `helper-allowlist.toml`, `lumen-helper.socket`/`.service`. `scripts/pi-build.sh`: sync to pitv and build there.

## Rules
- Build milestone by milestone and stop after each one for Laith to test on the TV.
- Lumen is pitv's boot session since M5 (2026-10-24): `lumen.service` enabled, Kodi is a Store app (M5.3), not a tile or the rescue; the rescue (3 crashes in 60 s) is a tty1 console login. Stop booting Lumen (console boot): `sudo sh ~/lumen-tv/session/install.sh --apply --revert && sudo reboot`.
- All Pi-specific code lives behind traits in `src/platform/`.
- Output defaults to 1080p60; 4K60 also works (`LUMEN_MODE=3840x2160@60` in `~/.config/lumen-tv/env`). Use the femtovg renderer. Follow the Slint rules in IN-PROGRESS.md (bake gradients/shadows into images, no animated shadow size/blur).
- Never run store install commands without showing them first.
