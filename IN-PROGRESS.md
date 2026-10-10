# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-10

## Where we are
M1.5 done (2026-10-08): Lumen's UI is now Slint (femtovg) on main, 60.0 fps / 0.1% slow at 1080p60 on the TV, no focus artifacts. Tauri, Svelte and WebKitGTK are gone. M2 closed on the TV remote. The input layer is **docked** (2026-10-09): keyboard is the dev control, no new input work until M5's input polish. Now on **M3, registry + launching + icons**, then M3.5 Settings/diagnostics. Kodi still boots by default; Lumen runs from `lumen-test.service`.

## Active Plan
Build M3 → M3.5 → M4 → M5 in order (see MILESTONES.md), stopping after each one for a TV test.

Planned layout:
```
lumen-tv/
  Cargo.toml, build.rs     one crate; slint-build compiles ui/
  src/
    main.rs                window setup, Slint callbacks, event bridge
    bench.rs               LUMEN_BENCH=1 frame-time bench
    input/                 evdev reader, hotplug, mapping loader, repeat/deadzone
    registry.rs            ~/.config/lumen-tv/apps/*.toml
    apps.rs                launch / track / close / lifecycle policies
    store.rs               manifest fetch, install/uninstall runner
    platform/mod.rs        traits: Windows, Display, Installer
    platform/pi.rs         labwc + foreign-toplevel, apt/flatpak, HDMI mode
  ui/                      Slint
    app.slint              root window, screen switching, spatial focus
    tile.slint
    home.slint, store.slint, details.slint, settings.slint
  config/input/            keyboard.toml, xbox.toml, playstation.toml, cec-remote.toml
  store/manifest.json      sample: Jellyfin, Plex, Steam Link
  session/                 labwc rc.xml + autostart, systemd/getty units, install.sh
  README.md, LICENSE
```

Decisions so far:
- Registry in `~/.config/lumen-tv/apps/` (XDG config dir: user-editable, survives reinstalls, no root needed). Mapping defaults ship in the app; user overrides go in `~/.config/lumen-tv/input/`.
- Build Rust natively on the Pi (simplest; no cross toolchain).
- Window control through labwc's wlr-foreign-toplevel-management protocol (activate/close), behind `platform::Windows`.
- Input → UI: evdev reader thread calls `slint::invoke_from_event_loop` to deliver normalized events to UI callbacks.

## Settled
- (2026-10-08) UI is Slint 1.18, femtovg renderer, winit backend under labwc. Not Skia: slower at both resolutions, and its default wgpu/Vulkan surface never fires the rendering notifier.
- (2026-10-08) Lumen is GPLv3 (Slint's royalty-free license excludes embedded systems).
- (2026-10-08) Port before M2. Kiosk browser for `fallback_url` decided in M3.
- (2026-10-08) 4K60 works with baked images (59.7 fps, 0.4% slow). Default output stays 1080p60 for now (Laith); `LUMEN_MODE=3840x2160@60` picks 4K.
- (2026-10-06) Kodi stays as a Lumen tile (close-on-exit, exclusive) for Games/YouTube/Mirror until replacements exist.
- (2026-10-06) Rust 1.99 via rustup (`~/.cargo/bin`) on the Pi and the Mac (not on the Mac's PATH). apt packages need Laith's sudo.
- (2026-10-06) Store manifest lives in this repo: github.com/Novapool/lumen-tv, `store/manifest.json` (raw URL once pushed).
- (2026-10-06) Pi 5 has 8 GB RAM. laith is already in the `input` group. TV native mode is 3840x2160; Lumen sets 1080p60 by default with `wlr-randr` (`LUMEN_MODE` overrides it).

- (2026-10-09) Clients run as children of Lumen in labwc (env XDG_RUNTIME_DIR=/run/user/1000, WAYLAND_DISPLAY=wayland-0, DISPLAY=:0 via Xwayland). All raise/close/fullscreen via foreign-toplevel:
  - Jellyfin: web client in Chromium (`http://192.168.5.174:8096`, app_id `lumen-jellyfin`), TV layout set once via `localStorage.layout = "tv"` in its profile (`~/.local/share/lumen-tv/browser/jellyfin`). Jellyfin Desktop (Flathub) was dropped as the launcher (still installed, for its icon): no GPU in its Mesa (software rendering, very slow) and its player showed subtitles with no video/audio.
  - Web apps get their own Chromium profile and app_id `lumen-<id>` (`{id}` in the browser command); otherwise a second launch hands off to the running Chromium and exits.
  - Plex: Chromium kiosk, app_id `lumen-plex`, ~1.7 GB, F1 opens Chrome help (needs an rc.xml swallow).
  - Steam Link: `steamlink` (Xwayland), app_id `SteamLink`; launcher script exits at once, real app `~/.local/share/SteamLink/bin/shell` reparented to PID 1.
  - Kodi: `kodi --windowing=wayland`, app_id `Kodi`, real app `kodi.bin`, close takes 4.5 s.

- (2026-10-09) Audio: PipeWire (`pipewire pipewire-pulse pipewire-alsa wireplumber`, user services) mixes for the HDMI output (vc4-hdmi-0, volume 100%). Without it Chromium's second audio stream got "Device or resource busy" and played silently (the Pi 5 HDMI PCM has no mixing). Kodi/Steam Link with pipewire-alsa still to be checked.
- (2026-10-09) Startup race: if Lumen starts before HDMI-A-1 is ready, `start-lumen.sh` can't set the mode and Lumen's window comes up tiny, top-left, not fullscreen (labwc "invalid output set for view"). Fixed in 2.1: `start-lumen.sh` waits up to ~5 s for the output, and Lumen asks for fullscreen on its own window if it comes up windowed.

- (2026-10-10) Registry (`~/.config/lumen-tv/apps/*.toml`, defaults in `config/apps/`, seeded on first run; fields in `src/registry.rs`, unknown keys rejected): `id`, `name`, `type` (`command`|`url`), `launch`, `url`, `fallback_url`, `background` (`keep-alive`|`close-on-exit`), `exclusive`, `icon`, `app_id`, `process` (real process name when the launcher exits at once, e.g. Steam Link's `shell`), `glyph`, `colors`, `order`. `url` apps run through `LUMEN_BROWSER` (default: Chromium kiosk with per-app profile and `--class=lumen-<id>`; `{id}` and `{url}` are substituted). Icon lookup: a path under `~/.config/lumen-tv/icons/`, else a hicolor theme name (svg, then largest png); on a miss the tile shows its glyph. No brand artwork ships: Plex keeps its glyph until Laith adds a file; Jellyfin's icon (`org.jellyfin.JellyfinDesktop`) resolves only while the Flathub Jellyfin Desktop stays installed.
- (2026-10-10) `session/labwc/rc.xml` swallows F1 and Super_L so clients (Chromium help) never see Home. Gate tools: `examples/press.rs`, `examples/toplevels.rs`.

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
- **M3 — Registry + launching + icons** (started 2026-10-08). Plan: `AGENT_JOBS.md` (transient).
  - Phase 0: done: `examples/toplevels.rs` + `examples/press.rs` (78822fd; foreign-toplevel v3, socket `wayland-0`, Lumen's window app_id "" / title "Lumen"). Client spike done (see Settled).
  - Phase 1: `platform::Windows` ∥ registry + lifecycle + process control ∥ icons (resolve + bake); input gating is a filter in main.rs (2.1), no `src/input/` changes.
  - Phase 1: done and merged (Windows 5b03651, registry/lifecycle 66c4ac6, icons 74e5c6c; 76 tests pass on the Pi). Jellyfin switched to its web client (3220d84).
  - Phase 2 (Job 2.1) done: registry, lifecycle, windows, icons and the Home gate are wired into main (`src/session.rs` holds the glue). Smoke test passed on the Pi (launch, F1 back, same PID on re-select, exclusive Steam Link ends Jellyfin). Phase 3: review/simplify/tests/docs (review fixes in 7a8870c). Phase 4: gate G1–G6 on the Pi, then at the TV.
  - Every app runs as a child of Lumen inside labwc (no systemd units). While an app is in front, Lumen acts on Home alone (keyboard).
  - **Review 3.1 (2026-10-10): Patch and ship.** All findings FIXED in 7a8870c (retiring runs, Home fallback via `Lifecycle::home(lumen_activated)`, no private Slint API, 1 s fullscreen timer, `libc::kill`) except one SKIPPED nit (no brand artwork in the repo to ship as `icons/jellyfin.svg`). FYI: bench `pass` judges only the 30 s motion phase; report idle next to motion in G7.

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

## Blockers
- (none)
- Bench is thermally sensitive: the Pi has no fan; after long builds it throttles at 75-82 C and 4K drops to 45-55 fps (a cool 4K run passed at 60). Let it cool before benching.

## Next Steps
1. M3 Phase 4 (2026-10-10): automated gate G1–G7 **passed** on the Pi (Home 2–7 ms, reselect same PID 5/5, exclusive kill 1.2 s, kill→Lumen 3–41 ms, guard OK, quick reselect OK; bench 60 fps / 0.1% at 1080p and 4K, Pi 66–69 °C). Laith's TV check (Job 4.3 in AGENT_JOBS.md) in progress:
   - **Bug: F1 reaches Chromium** (Jellyfin lands on the Google help page). Cause: in labwc `<action name="None"/>` clears a binding instead of swallowing the key. Fix `session/labwc/rc.xml`: bind F1 and Super_L to a no-op action (e.g. `<action name="Execute" command="true"/>`), restart lumen-test, verify Lumen still logs `[input] Home` and Chromium's title stays put. Recover a stuck page with Ctrl+W (closes the help tab).
   - Audio after reboot: WirePlumber offered only "Dummy Output" (HDMI jack reads unplugged until a mode is set). Fixed with `../config/wireplumber-51-pitv-hdmi.conf` (see `../CLAUDE.md` Audio); confirm after the next reboot that Kodi and Lumen have sound. Check Screen Mirror (UxPlay writes to the HDMI ALSA device directly).
   - Still to check at the TV: icons sharp; Jellyfin F1 → Lumen with running dot → reselect resumes; Steam Link stream smooth with sound, F1 ends it; Claude kills Jellyfin from SSH while Laith watches.
   - Then Job 4.4: close M3 (MILESTONES result line, Recently Completed, delete AGENT_JOBS.md, remove `.claude/worktrees/`).
2. Parked for M5 input polish (don't work on these before then): CEC Back/Home (`evtest /dev/input/event5` + `sudo cec-ctl -d /dev/cec0 -M`, try `--vendor-id 0x00e091`), remote Home fallback, PS5 controller over USB/BT.

## Recently Completed
- 4K bake merge (2026-10-08): `exp/baked-hero` merged into main (0abbe64). 4K60 59.7 fps / 0.4% > 25 ms / p95 17.4 ms (was 27.8 fps); 1080p60 60.0 / 0.1% / p95 17.1. M2 input kept; HDMI remote verified at 4K. Default stays 1080p. Branch, worktree and `~/lumen-bake` on the Pi removed.
- M1.5 Slint port (2026-10-08): gate passed (60.0 fps, 0.1% > 25 ms, Laith: no artifacts). Spike branch tagged `spike-slint-2026-10-08`, worktree and Pi test files removed.
- Slint spike (2026-10-08): passed its gate at 1080p (femtovg 60.0 fps, 0.1% slow, no flash); 4K too slow with live effects (fixed later by baking). Laith chose Slint. Report: `git show spike-slint-2026-10-08:spike-slint/REPORT.md`.
