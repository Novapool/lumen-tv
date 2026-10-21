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
- (none) M4 closed 2026-10-20. Next: plan M4.5.

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
