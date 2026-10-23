# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-20

## Where we are
Lumen's UI is Slint (femtovg), 60 fps at 1080p60 and 4K60 on the TV. Done: M3 (registry, icon tiles, Jellyfin/Steam Link/Kodi launch, Home from any app), M3.5 (Settings/diagnostics; the LG remote drives everything, red = Home), M4 (Store: flatpak `--user` installs from `store/manifest.json`, live tiles) and M4.5 (2026-10-20): Settings → Bluetooth pairs the PS5 DualSense with the remote alone; the pad drives Lumen, Jellyfin (as keys) and Steam Link (directly). **Now: M5 boot session + polish** (started 2026-10-21, ahead of the root install helper (PIN) milestone, which MILESTONES.md still lists first; confirm/record the order). Kodi still boots by default; Lumen runs from `lumen-test.service`.

## Active Plan
Order: M5 → root helper → M5.5 → M6 (see MILESTONES.md), stopping after each one for a TV test.

**M5 (plan in AGENT_JOBS.md, transient).** Laith is away from the TV, so this round builds and checks only on the Mac and over ssh; the Gate and all TV checks are the T0-T14 checklist at the end of AGENT_JOBS.md.
- Phase 1 (parallel): 1.1 stick rebound lockout (`src/input/mapping.rs`, `devices.rs`, fake-pad `flick`) · 1.2 `session/lumen.service` + `session/install.sh` (dry run by default; `--apply`, `--set-default`, `--revert`, `--quiet-boot`) + saved mode in `start-lumen.sh` · 1.3 `src/settings.rs` (`~/.config/lumen-tv/settings.toml`: `mode`, `store_url`) + `src/platform/output.rs` (re-applies the saved mode when HDMI-A-1 reappears) + store URL precedence.
- Phase 2: 2.1 Settings UI: Toggles → Resolution (1080p60/4K60), Mappings panel, About panel (version/commit, manifest URL), build info; first Pi build.
- Phase 3 (parallel): review, simplify, test gaps, README.md.
- Phase 4: tester: Mac build/test/clippy, Pi build/test/examples, `install.sh` dry run + `systemd-analyze verify` on the Pi. Kodi stays enabled.
- Not now (needs Laith): installing lumen.service, switching the boot default, any sudo, the visual polish pass, the Gate.

Layout (as built):
```
lumen-tv/
  Cargo.toml, build.rs     one crate; slint-build compiles ui/
  src/
    main.rs                window setup, Slint callbacks, Settings wiring, input gate, bench hook
    session.rs             glue: lifecycle + windows + tiles (UI thread)
    apps.rs                Lifecycle planner (pure, tested) + process control, MPRIS pause
    registry.rs, icons.rs  app TOMLs; icon lookup + bake onto tiles
    bake.rs, bench.rs      CPU-baked images; LUMEN_BENCH frame-time bench + FpsMeter
    diag.rs, logtail.rs    Settings panel data; capture of Lumen's own log for the Log panel
    reclaim.rs             re-claim the CEC remote after an app exits (debounced)
    home.rs                Home tiles/heroes; live reload from the registry
    store.rs, storefeed.rs Store: manifest, step runner, receipts; Slint wiring
    bluetooth.rs, btfeed.rs Bluetooth panel: pure parsing/ordering/errors; Slint wiring
    input/keys_out.rs      controller → keys for web apps (uinput "lumen-keys")
    platform/bluez.rs      BlueZ via busctl + scan_slot.rs/agent_slot.rs (bluetoothctl children); fake_bt.rs = LUMEN_BT=fake
    input/                 evdev reader, hotplug, mapping loader, repeat/deadzone, dispatch
    platform/mod.rs        traits: Windows, Tv
    platform/pi.rs         CEC remote claim (cec-ctl) + OK/Back keymap remap
    platform/toplevel.rs   labwc wlr-foreign-toplevel
  ui/                      app.slint (screens, focus), settings.slint, bluetooth.slint, store.slint, details.slint, tile.slint, theme.slint
  config/input/            keyboard, xbox, playstation, gamepad, cec-remote (.toml)
  config/apps/             default registry entries; config/kodi-tile = Kodi tile launcher (CEC off)
  examples/                fake-pad.rs, press.rs, toplevels.rs (gate tools)
  store/manifest.json      the Store's app list (served from GitHub raw)
  session/                 labwc rc.xml, start-lumen.sh, lumen-test.service, sudoers
  scripts/pi-build.sh      rsync to pitv + cargo build --release there
```

Decisions so far:
- Registry in `~/.config/lumen-tv/apps/` (XDG config dir: user-editable, survives reinstalls, no root needed). Mapping defaults ship in the app; user overrides go in `~/.config/lumen-tv/input/`.
- Build Rust natively on the Pi (simplest; no cross toolchain).
- Window control through labwc's wlr-foreign-toplevel-management protocol (activate/close), behind `platform::Windows`.
- Input → UI: evdev reader thread calls `slint::invoke_from_event_loop` to deliver normalized events to UI callbacks.

## Settled
- (2026-10-08) UI is Slint 1.18, femtovg renderer, winit backend under labwc. Not Skia: slower at both resolutions, and its default wgpu/Vulkan surface never fires the rendering notifier.
- (2026-10-08) Lumen is GPLv3 (Slint's royalty-free license excludes embedded systems).
- (2026-10-08) Port before M2. Kiosk browser for `fallback_url` = Chromium (M3).
- (2026-10-08) 4K60 works with baked images (59.7 fps, 0.4% slow). Default output stays 1080p60 for now (Laith); `LUMEN_MODE=3840x2160@60` picks 4K.
- (2026-10-06) Kodi stays as a Lumen tile (close-on-exit, exclusive) for Games/YouTube/Mirror until replacements exist.
- (2026-10-06) Rust 1.99 via rustup (`~/.cargo/bin`) on the Pi and the Mac (not on the Mac's PATH). apt packages need Laith's sudo.
- (2026-10-06) Store manifest lives in this repo: github.com/Novapool/lumen-tv, `store/manifest.json` (raw URL once pushed).
- (2026-10-06) Pi 5 has 8 GB RAM. laith is already in the `input` group. TV native mode is 3840x2160; Lumen sets 1080p60 by default with `wlr-randr` (`LUMEN_MODE` overrides it).

- (2026-10-09) Clients run as children of Lumen in labwc (env XDG_RUNTIME_DIR=/run/user/1000, WAYLAND_DISPLAY=wayland-0, DISPLAY=:0 via Xwayland). All raise/close/fullscreen via foreign-toplevel:
  - Jellyfin: web client in Chromium (`http://192.168.5.174:8096`, app_id `lumen-jellyfin`), TV layout set once via `localStorage.layout = "tv"` in its profile (`~/.local/share/lumen-tv/browser/jellyfin`). Jellyfin Desktop (Flathub) was dropped as the launcher (still installed, for its icon): no GPU in its Mesa (software rendering, very slow) and its player showed subtitles with no video/audio.
  - Web apps get their own Chromium profile and app_id `lumen-<id>` (`{id}` in the browser command); otherwise a second launch hands off to the running Chromium and exits.
  - Plex: Chromium kiosk, app_id `lumen-plex`, ~1.7 GB (F1 is swallowed in rc.xml, so Chrome help no longer opens).
  - Steam Link: `steamlink` (Xwayland), app_id `SteamLink`; launcher script exits at once, real app `~/.local/share/SteamLink/bin/shell` reparented to PID 1.
  - Kodi: `kodi --windowing=wayland`, app_id `Kodi`, real app `kodi.bin`, close takes 4.5 s.

- (2026-10-09) Audio: PipeWire (`pipewire pipewire-pulse pipewire-alsa wireplumber`, user services) mixes for the HDMI output (vc4-hdmi-0, volume 100%). Without it Chromium's second audio stream got "Device or resource busy" and played silently (the Pi 5 HDMI PCM has no mixing). Kodi/Steam Link with pipewire-alsa still to be checked.
- (2026-10-09) Startup race: if Lumen starts before HDMI-A-1 is ready, `start-lumen.sh` can't set the mode and Lumen's window comes up tiny, top-left, not fullscreen (labwc "invalid output set for view"). Fixed in 2.1: `start-lumen.sh` waits up to ~5 s for the output, and Lumen asks for fullscreen on its own window if it comes up windowed.

- (2026-10-10) Registry (`~/.config/lumen-tv/apps/*.toml`, defaults in `config/apps/`, seeded on first run; fields in `src/registry.rs`, unknown keys rejected): `id`, `name`, `type` (`command`|`url`), `launch`, `url`, `fallback_url`, `background` (`keep-alive`|`close-on-exit`), `exclusive`, `icon`, `app_id`, `process` (real process name when the launcher exits at once, e.g. Steam Link's `shell`), `glyph`, `colors`, `order`. `url` apps run through `LUMEN_BROWSER` (default: Chromium kiosk with per-app profile and `--class=lumen-<id>`; `{id}` and `{url}` are substituted). Icon lookup: a path under `~/.config/lumen-tv/icons/`, else a hicolor theme name (svg, then largest png); on a miss the tile shows its glyph. No brand artwork ships: Plex keeps its glyph until Laith adds a file; Jellyfin's icon (`org.jellyfin.JellyfinDesktop`) resolves only while the Flathub Jellyfin Desktop stays installed.
- (2026-10-10) `session/labwc/rc.xml` swallows F1 and Super_L so clients (Chromium help) never see Home. The action must be `Execute command="true"`, since `None` clears the binding. Gate tools: `examples/press.rs`, `examples/toplevels.rs`.
- (2026-10-13) Home from a keep-alive app sends MPRIS Pause (`busctl --user`, players owned by the app's process tree). Reselect does not auto-resume (Laith).

- (2026-10-13) TV remote (LG basic, SIMPLINK on): Lumen claims `/dev/cec0` as plain Playback "Lumen" (`cec-ctl --playback --osd-name Lumen`, then active source 2.0.0.0); the LG vendor ID 0x00e091 forwarded nothing extra. Home/Settings/Exit and a held Back never reach the Pi (the TV opens its own menu), so **red = Home** (KEY_RED 398 > 247, never reaches XKB, no rc.xml swallow needed); keyboard F1 stays Home.
- (2026-10-13) Kernel keymap remap on every claim (EVIOCSKEYCODE_V2 on `vc4-hdmi-0`): OK 0x00 KEY_OK→KEY_ENTER, Back 0x0d KEY_EXIT→KEY_BACK, so Chromium/Jellyfin web act on them. `cec-remote.toml` lists post-remap codes plus KEY_OK/KEY_EXIT as a fallback.
- (2026-10-13) Kodi tile runs through `~/.local/bin/kodi-tile` (own home `~/.kodi-tile`, userdata linked to `~/.kodi`, CEC peripheral forced off) so Lumen keeps the remote; the boot Kodi is untouched. After any app exits Lumen re-claims the remote + active source (`src/reclaim.rs`). Backup `~/.kodi-backup-20261013`.
- (2026-10-13) Plex removed (Laith): Jellyfin covers it.

- (2026-10-20) Store: manifest `store/manifest.json` (version 1; each app = registry fields + `description`, `detail`, `install`, `uninstall`, `root`), fetched from `LUMEN_STORE_URL` (default `https://raw.githubusercontent.com/Novapool/lumen-tv/main/store/manifest.json`; a `/path` or `file://` reads from disk). `lumen-tv --store-check [source]` prints it. Steps run as `sh -c` as laith, one job at a time, stop at the first failure, no rollback. Registry TOML + receipt (`~/.config/lumen-tv/store/<id>.json`, keeps the uninstall steps used) are written only after every step passes.
- (2026-10-20) Root policy (Laith): the Store runs user-level steps only (flatpak `--user`); `root: true` entries show their commands to run over SSH. Everything is uninstallable, Jellyfin included.
- (2026-10-20) Lumen clears ambient capabilities for every child (`apps::drop_ambient_caps`): lumen-test's PAM session gives it CAP_WAKE_ALARM, and bwrap (Flatpak) refuses to run with it.
- (2026-10-20) KolourPaint (the M4 gate app): app_id `org.kde.kolourpaint`, no `process` needed (Lumen tracks bwrap; the app is its child), icon from `~/.local/share/flatpak/exports`.

- (2026-10-20) Bluetooth (M4.5): no D-Bus crate. Snapshot = `busctl --list tree org.bluez` + per-object `get-property --json=short` (GetManagedObjects JSON fails on ManufacturerData's integer keys). Scan = a held `bluetoothctl --timeout 60 scan on` child; it stops after 60 s, when leaving the device rows, on panel/Settings close, and before pair/connect (shared Wi-Fi/BT radio). Pairing needs a default agent: while the panel is open Lumen holds a bluetoothctl session (`agent off` → `agent NoInputNoOutput` → `default-agent`); pair = Trusted → Pair → Trusted again → check Bonded → Connect. Without the agent the DualSense paired but didn't bond and BlueZ refused its HID link. Battery: BlueZ Battery1, else `/sys/class/power_supply/*<mac>*/capacity`. Device list: named only, paired → gamepads → signal, max 20, stable positions. No sudo needed (laith reaches org.bluez).
- (2026-10-20) DualSense: main node → `playstation.toml` (Cross Select, Circle Back, Options Menu, PS Home, left stick + d-pad, deadzone 0.4); Touchpad and Motion Sensors nodes are ignored by Lumen (the touchpad still moves labwc's pointer; Laith kept it on). Home rules: a PS tap is Home except inside Steam Link, where a held PS (≥ 0.8 s) is Home and a tap belongs to Steam; a controller's held Circle is plain Back while any app is in front (Home only in Lumen's own screens); keyboard F1 and remote red unchanged.
- (2026-10-20) While a `type = "url"` app is in front, controller actions become keys on Lumen's uinput keyboard "lumen-keys" (arrows, Enter, KEY_BACK, play/pause); Lumen's reader ignores that device. Steam Link and Kodi read the pad themselves.

## Slint rules (from the spike)
- Never animate a drop shadow's size or blur: the blur re-renders every frame (56.5 fps, 5.2% slow). Keep the shadow box fixed and fade it with `opacity`. Slint draws no shadow for an unfilled Rectangle, so fill it and hide it behind the face.
- Slint redraws only on change: frame stats count only frames while something is animating.
- No "hold" animation workaround needed (that was a WebKit compositor bug).
- Radial gradients are circles only.
- Bake gradients, gloss and shadows into images (`src/bake.rs`); the Pi 5 GPU is fill-rate bound and can't draw them live at 4K.
- Slint drops a texture once nothing shows it, so keep the images that change per focus move small (heroes are 640×234 and scaled up).
- Never crossfade two full-screen layers on the GPU: blend the two baked backdrops on the CPU (`bake::blend`) and draw one Image.
- `if`-created drop-shadow elements re-blur every time they are created; use one fixed shadow Image and fade it.

## Current Work
- M5 (2026-10-21): planned in AGENT_JOBS.md; Phase 1 not started. Settings contract: mode precedence settings.toml `mode` > `LUMEN_MODE` > 1080p60; store precedence settings.toml `store_url` > `LUMEN_STORE_URL` > default.

### Review — M5 Phases 1-2 (Job 3.1, 2026-10-21)
**Summary:** Scope is complete and mostly solid: the watcher can't loop (3-attempt cap, reset only on reappear/new target, never applies when the mode matches), settings.toml is written atomically in the exact `mode = "…"` line and start-lumen.sh validates it, install.sh is a real dry run with validated user/home and visudo before install. lumen.service with `Restart=always` + `StartLimitBurst=3/60s` + `OnFailure=kodi.service` + `Conflicts=` is correct (see FYI). Four behaviour bugs need a patch before the TV session: a held stick reversal can go dead, the Resolution toggle can get stuck, a given-up watcher reads "applying…" forever, and a broken settings.toml floods the Log panel. Not ready for TV testing until those are fixed.

**Required**
1. `src/input/mapping.rs` `Axis::update` + `src/input/devices.rs` `run`: a suppressed reversal fires only on the next event from that axis. At full deflection a DualSense stick reads a steady 0/255, and the kernel input core drops repeated identical ABS values, so no further event arrives. Right flick, then a fast push to full Left within 150 ms of the release: Left never fires (and never auto-repeats) until the thumb moves. Release happens at |pos| < 0.3, so a quick reversal reaches the limit well within 150 ms. Fix: keep the last `pos` on `Axis`, add `DeviceMap::tick(now) -> Vec<(Action,bool)>` that presses any axis whose lockout has expired while it is still past the deadzone, expose `DeviceMap::deadline()`, and use `min(dispatch.deadline(), map deadlines)` in the reader's `recv_timeout`. Add a test: 255@0, 128@20, 0@40, then `tick(ms(171))` → Left press. (Builder item 4: this is a real bug, not acceptable as is.)
2. `src/main.rs` `on_toggle_resolution`: "current" is re-read from settings.toml, and a failed save is only `println!`ed. With an unreadable or broken settings.toml (`load` → defaults, `save_mode` refuses to overwrite), every press computes 1080p → 4K, so you can switch to 4K but never back from the UI. Fix: toggle from `platform::output::status().desired` (the live target), and show the save error in the Resolution row's detail (e.g. "4K60 · not saved: …", ERROR level).
3. `src/diag.rs` `resolution_detail` reads `last_result` text for "fail"/"error". `output::apply` errors look like `wlr-randr 3840x2160@60: <first stderr line>` or `wlr-randr: No such file or directory`, which usually contain neither. After the 3-attempt cap the row says "applying…" forever. Fix: carry the outcome as a type. Add `last_ok: Option<bool>` and `gave_up: bool` (from `state.attempts >= MAX_ATTEMPTS`) to `DisplayStatus`, map them to "failed: …"/"gave up after 3 tries" at ERROR, and drop the substring match.
4. `src/main.rs` `Sources::panels` (runs every 1 s while Settings is open): it calls `settings::mode()`, `store::source_with_origin()` and `settings::load()`, so it reads settings.toml 3×/s, and with a broken file the first two each `eprintln!("[settings] …")`. That is 2 log lines/s into logtail, so the Log panel fills with them during exactly the T6 check. Fix: `load()` once per refresh, pass the `Settings` into `resolve_mode`/`resolve_store_url` (they are already pure), and log a settings error only when it changes (keep the last one in `Sources`).
5. `src/diag.rs` `SHIPPED_MAPPINGS` is a copy of `input::mapping::SHIPPED` (which is already `pub`; only `mod mapping` is private). A 6th shipped file added to one list makes the Mappings panel wrong. Fix: `pub use mapping::SHIPPED;` in `src/input/mod.rs`, then use `input::SHIPPED` in main.rs/diag.rs and delete `SHIPPED_MAPPINGS` + `shipped_has`'s copy (`shipped_has` can take the slice). (Builder item 3: agreed.)

**Nit / FYI**
- Nit `src/settings.rs`: `save_store_url` is unused (nothing in the UI writes `store_url`; About says to edit the file). Delete it. Keep `save_store_url_to` only if tests need it, or write the fixture with `write_to`. (Builder item 5.)
- Nit `session/start-lumen.sh`: the sed only matches the exact `mode = "…"` line. A hand-edited `mode="3840x2160@60"` or `mode = '…'` is valid TOML that Lumen accepts, but the script ignores it, so the session starts at the env/default mode and the watcher switches about 2 s later (a visible mode flash). Use `s/^[[:space:]]*mode[[:space:]]*=[[:space:]]*["']\([^"']*\)["'].*/\1/p`.
- Nit `build.rs`: no `rerun-if-changed=.git/HEAD`, so Mac builds keep a stale commit and date after a new commit, and Pi builds keep the old build date when `LUMEN_GIT` is unchanged. Add `cargo:rerun-if-changed=.git/HEAD` (+ `.git/index` for dirty state).
- FYI builder item 1 (now `Restart=always`), checked: (a) Lumen/labwc exiting, cleanly or by crash, restarts after 2 s. This matters because labwc `-S` exits 0 when Lumen dies, so `on-failure` would never have fired. (b) A stop caused by `Conflicts=` (starting kodi/steamlink/uxplay/lumen-test, or `systemctl stop`) is a stop job and is not restarted, so there is no loop. Conflicts work in both directions. (c) `StartLimitBurst=3` counts the first start, so the 3rd exit within 60 s ends in `start-limit-hit` → `failed` → `OnFailure=kodi.service` runs. Lumen is already inactive, so `Conflicts=kodi` doesn't block it. (d) Lumen has no Power/exit action except Ctrl+Q on a keyboard, which now just restarts it. Two corrections for T11/README: SEGV ×3 (not ×4) triggers the rescue, and after `start-limit-hit` a plain `systemctl start lumen.service` is refused for 60 s. Use `sudo systemctl reset-failed lumen.service && sudo systemctl start lumen.service`. In rescue Kodi, the Steam Link/Screen Mirror favourites return to Kodi, not Lumen (their ExecStopPost); that's fine until M6.
- FYI builder item 2: nothing in Lumen calls `systemctl` (grep: no hits in src/ or ui/). Kodi and Steam Link run inside the labwc session, and with `Restart=always` a "restart Lumen" action is just exiting. So sudoers-lumen needs no lumen.service rights. Note: install.sh never updates `/etc/sudoers.d/lumen` once it exists, so a later change to `sudoers-lumen` needs a manual step. Also, the cmdline.txt write is `cat tmp > cmdline` (truncate, then write) on FAT /boot; writing a temp file in `/boot/firmware` and `mv` over it is safer.

**What works well:** `output::decide` is a pure, well-tested state machine. The watcher thread only runs commands, and `request_mode` drains to the newest target. `settings::update` refuses to save over a file it can't parse, so a typo never wipes `store_url`. install.sh's `run()` wrapper makes the dry run structural rather than per-step, and the user/home charset checks close the sed-substitution injection path before the unit and sudoers are rendered.

**Verdict:** Patch and ship. Fix Required 1-5 (owners: 1 → Job 1.1; 2, 4 → Job 2.1; 3 → Jobs 1.3 + 2.1; 5 → Jobs 1.1 + 2.1), then Phase 4.

### Test Results — Job 4.1 (tester, 2026-10-21)
**Verdict: FAIL on one check (step 6, Kodi active). Every build, test and lint step passed.** Code is not changed; the Kodi state is the running `lumen-test.service` (see below), not a build error. No units were started, stopped, enabled or disabled, no sudo, no `install.sh --apply`.

| # | Command | Exit | Result |
|---|---|---|---|
| 1a | Mac `~/.cargo/bin/cargo build` | 0 | Finished dev profile, no errors |
| 1b | Mac `~/.cargo/bin/cargo test` | 0 | `test result: ok. 280 passed; 0 failed; 2 ignored` |
| 2 | Mac `~/.cargo/bin/cargo clippy --all-targets` | 0 | 5 warnings (not errors): `src/bluetooth.rs:432` div_ceil; `src/icons.rs:42` and `:70` large size difference; `src/input/mapping.rs:296` unnecessary closure (**file changed since 4fb802d**); `src/store.rs:463` is_multiple_of (**file changed since 4fb802d**) |
| 3 | Mac `sh -n session/start-lumen.sh` / `sh -n session/install.sh` | 0 / 0 | Both parse (install.sh has `#!/bin/sh`). `shellcheck` is not installed, so not run |
| 4a | `scripts/pi-build.sh` (rsync + `cargo build --release`) | 0 | `Finished release profile [optimized] in 1m 57s`, no errors or warnings. `devices.rs` and `output.rs` compiled on the Pi with no errors |
| 4b | `ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo test --release'` | 0 | `test result: ok. 288 passed; 0 failed; 2 ignored` |
| 4c | `ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo build --release --examples'` | 0 | `Finished release profile` (examples already built by 4b; no errors) |
| 5a | `./target/release/lumen-tv --store-check` | 0 | `4 apps, 0 errors` |
| 5b | `sh session/install.sh` (dry run) | 0 | Changed nothing. Plan: apt install list (labwc wlr-randr xwayland chromium flatpak v4l-utils bluez pipewire pipewire-pulse pipewire-alsa wireplumber build-essential pkg-config rsync curl + dev libs); groups already set; uinput already OK; would install `/etc/systemd/system/lumen.service`; sudoers already installed. Summary: "Boot default unchanged. lumen.service is installed but not enabled" (that summary line is printed by the script's text, not by a real state check) |
| 5c | `systemd-analyze verify ~/lumen-tv/session/lumen.service` | 0 | No output (no warnings) |
| 5d | `cat ~/.config/lumen-tv/env` | 0 | Empty or absent: `LUMEN_MODE` / `LUMEN_STORE_URL` are not set there |
| 5e | `ls ~/.config/lumen-tv/settings.toml` | 2 | `No such file or directory` (expected, not created yet) |
| 6a | `systemctl is-enabled kodi.service` | 0 | `enabled` |
| 6b | `systemctl is-active kodi.service` | **3** | `inactive` (**expected Kodi to be active; it is not**) |
| 6c | `systemctl is-enabled lumen.service` | 4 | `not-found` (correct: lumen.service is not installed) |

**Why Kodi is inactive (step 6b):** `lumen-test.service` is `active (running)` since 23:23:35, and its `Conflicts=kodi.service` stopped Kodi at that same second (journal: `Stopping kodi.service` then `Stopped kodi.service` at 23:23:35). This is the test session Laith or the earlier job started, not a regression from the build. Kodi will not come back until `lumen-test` is stopped, which this job was not allowed to do. Reading state only, nothing was changed.

**Notes for the main conversation:**
- The Pi test count is 288 (last known 234), Mac is 280 (last known 226). Both are higher because the M5 tests were added.
- The `sh session/install.sh` dry run printed a temp path for the unit (`/tmp/tmp.*`), but that file did not exist afterwards, so nothing was left behind.
- Before the TV session, Laith must stop `lumen-test.service` (or let Kodi return) and then recheck Kodi is active. Step 6 should be rerun then.

## Known issues
- After `systemctl stop lumen-test`, its ExecStopPost starts Kodi: wait until kodi is active before starting a test unit again.
- `ssh pitv` uses the IP (192.168.1.60), since `pitv.local` (mDNS) doesn't resolve over the VPN.
- `/dev/uinput` is writable by the input group: `cargo run --release --example fake-pad [secs]` on the Pi tests the input layer without a controller.
- Perf lines log under `labwc[...]`, so grep the whole journal, not `-u lumen-test`.
- Don't run wf-recorder on the Pi: its frames come out garbled and it froze the TV output.
- History: the WebKit focus-change flash/snap (compositor drew pre-animation values for ~2 frames) was patched with "hold" animations (95dc6f9); gone since the Slint port.
- Benchmark: put `LUMEN_BENCH=1` in `~/.config/lumen-tv/env` (runs on every start; remove it after), or press B on the home screen.

- Home (`KEY_F1`/`KEY_LEFTMETA`) is read from evdev, so the focused app also receives it unless rc.xml swallows it (F1 and Super_L are; `KEY_HOMEPAGE` is not).
- Steam Link needs Xwayland, and labwc must be restarted after installing it (restart `lumen-test`).
- Flatpak apps: the launched process is a wrapper, so descendants (`bwrap`/app) may outlive close; set `process` and check the terminate path before shipping one.

- Output mode resets to 4K after the TV power-cycles: labwc drops and re-adds HDMI-A-1 at its preferred 3840x2160, and only `start-lumen.sh` applies `LUMEN_MODE` (1080p60 default), once at start. Found at the M3.5 TV check (2026-10-19). Lumen holds 60 fps at 4K, so this is a perf/consistency issue, not a breakage. Fix candidate: re-apply the mode when the output reappears.

- DualSense reconnect: once at the M4.5 TV test, the first PS press after turning the pad off never reached the Pi (no BlueZ log); the next presses reconnected in 5-6 s. Watch it; an M5 option is asking BlueZ to connect known pads.

- Stick rebound (Laith, 2026-10-20): flicking the left stick right/left on Home moves focus, then the stick's spring-back overshoots past centre and fires the opposite direction, so focus snaps back. Not fixed; scheduled for M5 input polish (opposite-direction lockout ~150 ms after release, or hysteresis on release).

## Blockers
- (none)
- Bench is thermally sensitive: the Pi has no fan; after long builds it throttles at 75-82 C and 4K drops to 45-55 fps (a cool 4K run passed at 60). Let it cool before benching.

## Next Steps
0. **M5:** run AGENT_JOBS.md Phases 1-4, then the T0-T14 TV checklist with Laith (boot switch only with his OK; `install.sh --apply --revert` undoes it). Delete AGENT_JOBS.md after execution.
1. **Root install helper (PIN) milestone (after M5):** project-director writes its MILESTONES entry + Gate from the 2026-10-19 scope change (root daemon, on-screen PIN pad, argon2 + rate limit, root-owned allowlist on the Pi), then orchestrator plans it.
2. Confirm the proposed M5.5 and M6 Gate lines with Laith before either starts (M6 is settled as Pi 5 only, `.deb` only).
3. Untested: a game in Steam Link with the pad on BT; uninstalling a keep-alive app while it runs (M4's 8 s wait). Check both once.

## Recently Completed
- M4.5 Controllers + Bluetooth (2026-10-20): gate passed at the TV (pair from the panel with the remote, pad drives Lumen/Jellyfin/Steam Link, forget, 0 drift events; G4 first reconnect flaky once). Fixed on the way: per-property BlueZ snapshot, default pairing agent (bonding), controller keys for web apps. Result line in MILESTONES.md, execution notes in docs/ARCHIVE.md.
- M4 Store (2026-10-20): gate passed on the Pi and at the TV (KolourPaint install → tile live → launch → uninstall; failing install shows its error and writes nothing). Fixed on the way: Flatpak apps couldn't start from Lumen (ambient caps). Result line in MILESTONES.md, execution notes in docs/ARCHIVE.md.
- M3.5 Settings/diagnostics + TV remote (2026-10-19): gate passed on the Pi and at the TV (all S/R/G checks; red = Home 5/5 from Jellyfin, Kodi, Steam Link; remote works after Kodi closes). Result line in MILESTONES.md, execution notes in docs/ARCHIVE.md.
