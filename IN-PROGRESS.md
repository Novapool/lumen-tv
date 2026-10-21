# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-20

## Where we are
Lumen's UI is Slint (femtovg), 60 fps at 1080p60 and 4K60 on the TV. Done: M3 (registry, icon tiles, Jellyfin/Steam Link/Kodi launch, Home from any app), M3.5 (Settings/diagnostics; the LG remote drives everything, red = Home) and M4 (2026-10-20): the Store tile lists `store/manifest.json` from GitHub, shows every command, installs/uninstalls flatpak `--user` apps with live progress, and Home updates without a restart. **Next: M4.5 controllers + Bluetooth pairing screen.** Then the root install helper (PIN) and M5 boot session (with a 1080p/4K switch). Kodi still boots by default; Lumen runs from `lumen-test.service`.

## Active Plan
Build M4.5 → root helper → M5 in order (see MILESTONES.md), stopping after each one for a TV test.

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
    input/                 evdev reader, hotplug, mapping loader, repeat/deadzone, dispatch
    platform/mod.rs        traits: Windows, Tv
    platform/pi.rs         CEC remote claim (cec-ctl) + OK/Back keymap remap
    platform/toplevel.rs   labwc wlr-foreign-toplevel
  ui/                      app.slint (screens, focus), settings.slint, store.slint, details.slint, tile.slint, theme.slint
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
- M4.5 planned (2026-10-20) in `AGENT_JOBS.md`. Gate confirmed by Laith; DualSense only (no Xbox pad). Next: Phase 0 (read-only Pi facts F1-F7, decisions D2-D8 with Laith), then Job 1.0.
- Phase 0 done (2026-10-20): no sudo needed (laith reaches org.bluez); answers D2 a (no crate), D3 a (Settings → Bluetooth), D4 b (held PS = Home inside Steam Link only), D5 c (touchpad left on), D6/D7 defaults; D8 pending before Phase 4. Details in AGENT_JOBS.md Phase 0.
- Job 1.0 done (2026-10-20, Mac 152 tests): `ui/bluetooth.slint` (BtDevice, BtState, BluetoothPanel), AppWindow `bt-*` + `nav-trace` (fired from a new `nav(e)` wrapper around `nav-step`), Bluetooth panel at `panels.length`, Toggles at `+1`; `platform::Bluetooth` trait (`snapshot()` returns `Result`) + BtSnapshot/BtAdapter/BtDeviceInfo; `src/bluetooth.rs` stub.
- Job 1.3 done (2026-10-20, Mac 156 tests; fake-pad's Linux code not compiled yet): `playstation.toml` unchanged in behaviour (Cross Select, Circle Back, Options Menu, PS Home, left stick + d-pad, deadzone 0.4; header documents codes/names/why Touchpad + Motion nodes are excluded). 4 DualSense tests in mapping.rs (USB/BT/Edge match, touchpad/motion get no mapping, buttons/hat, 0..255 rest/drift gives nothing). `fake-pad dualsense [secs]` creates main/Touchpad/Motion nodes, runs a Home-safe script incl. drift + unplug/replug. Noticed: mapping.rs test helper `fake_resolve` hashes unknown names (possible collisions).
- Job 1.1 done (2026-10-20, Mac check/test pass, GUI not run): Bluetooth panel levels 1 rows (Scan row + devices; busy rows ignored), 2 action sheet (snapshot `bt-item`; Pair/Cancel or Connect|Disconnect/Forget/Cancel), 3 confirm (Cancel first) for pair/forget; connect/disconnect fire straight from the sheet. `bt-changed` fires only on show/hide transitions. Additive contract: BluetoothPanel `item`/`pending`, global `BtText`. `nav-trace` sends state lines (`home focus=<id>`, `settings panel=… level=… row=… "…"`, `bt sheet …`, `bt confirm …`, `store level=… row=…`). For 2.1: replace the `bt-devices` model on refresh (the row clamp hangs on `changed`).
- Job 1.2 done (2026-10-20, Mac 172 tests; bluez.rs never compiled): pure `src/bluetooth.rs` (parse GetManagedObjects, D6 select/sort, kinds, battery: BlueZ Battery1 then `/sys/class/power_supply/*<mac>*/capacity`, `friendly_error`, `rows`/`view`, `ScanTimer`); `platform::bluetooth()` = BluezBt on Linux unless `LUMEN_BT=fake`; `fake_bt.rs` script (DualSense 2 s, Phone 1 3 s, "Fail Pad" 4 s fails pairing; pair 1.5 s → connected, battery 70); `bluez.rs` pair = Trusted → Pair (AlreadyExists ok) → Connect, scan = long-lived `bluetoothctl --timeout N scan on`. Backends return raw errors; 2.1 maps them with `friendly_error`.
  - Checked read-only on the Pi (main conversation): the real `busctl --json=short` GetManagedObjects shape matches the fixture (`{"type":"a{oa{sa{sv}}}","data":[{path:{iface:{prop:{type,data}}}}]}`); `--system --timeout=60 --json=short` before the subcommand is accepted. A call on a device object that doesn't exist fails with `Call failed: Method "Connect" with signature "" on interface "org.bluez.Device1" doesn't exist` (map to "device not found"). Open for 2.1/Phase 4: Connect right after Pair may say AlreadyConnected (tolerate it in `pair`), and whether `bluetoothctl --timeout N scan on` stays alive without a TTY (needs a real scan, D8).
- Job 2.1 done (2026-10-20, Mac 177 tests, Pi 184 incl. Linux-only; first Pi build clean after one fix, fake-pad `id.clone()`): `src/btfeed.rs` (1 s poll only while the panel shows, scan toggle, one pair/connect/disconnect/forget at a time with `busy` + friendly result, rows in place when the count is equal else a new model so the Slint clamp fires); `[nav]` lines only with input debug on; held-PS rule in `input/dispatch.rs` (`home_needs_hold`, `Dispatcher::press_in`), foreground id from `session::sync_tiles` via `input::set_foreground`; `LUMEN_BENCH=bt` (motion redraws continuously since Settings has no animations; `"screen":"bt"`); `--bt-check` (Pi: adapter powered, 0 devices); `friendly_error` maps "doesn't exist" and Already Connected; `bluez::pair` tolerates AlreadyConnected. Unverified at runtime: everything against real BlueZ (scan child without a TTY, Pair/Connect, battery), the held-PS rule on the Pi with a real pad, the bt bench numbers on the Pi, nav-trace in journal.
- Job 3.3 done: 11 tests added (every friendly_error branch, ScanTimer edges, select ties/cap, fake-backend failures, device_path injection on Linux); Mac 193 pass; no bugs. Not testable without a refactor: the one-action guard in `btfeed::act`, foreground change mid-hold.
- **M4.5 review (Job 3.1, 2026-10-20). Verdict: patch and ship.** Confirm/snapshot/one-action/address validation/off-UI-thread busctl/scan stop on close/Slint rules/privacy all pass.
  - H1 a gamepad's held Circle (long-press Back → Home, M2) closes Steam Link/Kodi mid-game. **Laith (2026-10-20): on a controller, held Circle is plain Back while any app is in front; in Lumen's own screens it stays Home.** M1 scan child is fire-and-forget (early exit undetected, stderr lost, zombie) and start/stop race on separate threads (a scan can outlive the closed panel). M2 Pair/Connect run while discovery is on (shared radio → ConnectionAttemptFailed); stop the scan first. M3 RSSI in the sort shuffles rows under focus every poll; keep a stable order.
  - Low: L1 busy note overwritten; L2 a stale in-flight poll beats the post-action re-poll; L3 failed pair leaves Trusted=true; L4 gamepad class by mapping file name (use `mapping::Class`); L5 "Stop scan" shown for discovery Lumen doesn't own; L6 fake-pad "safe on Home" comment wrong (Cross launches a tile); L7 `pair()` matches user-facing text. FYI: check in Phase 4 whether the bluetoothctl scan child registers an agent; the bt bench counts every frame (keep `screen=bt` separate from Home numbers).
- **M4.5 simplify (Job 3.2):** fold `f.error` into `view()`'s note and drop `BtView`; untangle `act`'s `Option<Option<…>>` and match "forget" explicitly; fake_bt `_at`/`new` not pub; drop `home_fires` (test the `home_needs_hold` table directly); `HELD_HOME_APP` private; optional `refresh(ui)` helper.
- Job 3.4 done (2026-10-20, Mac 210 tests, Pi 218 release, examples build, `--bt-check` ok): H1 gamepad long Back stays Back while an app is in front (`back_long_press_is_home`, `Dispatcher::press_rules`, `mapping::is_gamepad`); M1/M2/M3, L1-L7 and the simplify list applied (new `platform/scan_slot.rs` holds the scan child: wanted flag + stderr on early exit + `scan_error` in the snapshot; `bluetooth::classify`/`BtErr`, `stable_order`, `shown_note`). Still unverified on real BlueZ/TV: scan child stderr, Pair/Connect stopping discovery, stale-poll re-poll, held Circle in Steam Link.

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

## Blockers
- (none)
- Bench is thermally sensitive: the Pi has no fan; after long builds it throttles at 75-82 C and 4K drops to 45-55 fps (a cool 4K run passed at 60). Let it cool before benching.

## Next Steps
1. **M4.5 kickoff (orchestrator).** Read MILESTONES.md M4.5 + scope changes, BRIEF.md, this file's Settled / Slint rules / Known issues, and `../CLAUDE.md`; write AGENT_JOBS.md. Controllers (PS5 DualSense, Xbox) and a Bluetooth pairing screen in Settings.
2. Give the root install helper (PIN) its own MILESTONES entry with a Gate (project-director), slotted after M4.5.
3. Untested from M4: uninstalling a keep-alive app while it runs in the background (the 8 s wait before step 1). Check it once with Jellyfin.

## Recently Completed
- M4 Store (2026-10-20): gate passed on the Pi and at the TV (KolourPaint install → tile live → launch → uninstall; failing install shows its error and writes nothing). Fixed on the way: Flatpak apps couldn't start from Lumen (ambient caps). Result line in MILESTONES.md, execution notes in docs/ARCHIVE.md.
- M3.5 Settings/diagnostics + TV remote (2026-10-19): gate passed on the Pi and at the TV (all S/R/G checks; red = Home 5/5 from Jellyfin, Kodi, Steam Link; remote works after Kodi closes). Result line in MILESTONES.md, execution notes in docs/ARCHIVE.md.
- M3 Registry + launching + icons (2026-10-13): gate passed on the Pi and at the TV (Home 2–7 ms, same-PID reselect 5/5, exclusive kill 1.2 s, SSH kill → Lumen 23 ms, Steam Link smooth with sound). Late fixes: F1 swallowed in labwc (665c03d), Steam Link window swap (2458926), Home pauses media via MPRIS (bb61e09). Result line in MILESTONES.md.
