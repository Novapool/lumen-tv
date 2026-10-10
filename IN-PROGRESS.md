# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-09

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
  - Jellyfin: web client in Chromium (`http://192.168.5.174:8096`, app_id `lumen-jellyfin`), TV layout set once via `localStorage.layout = "tv"` in its profile (`~/.local/share/lumen-tv/browser/jellyfin`). Jellyfin Desktop (Flathub) was dropped: no GPU in its Mesa (software rendering, very slow) and its player showed subtitles with no video/audio.
  - Web apps get their own Chromium profile and app_id `lumen-<id>` (`{id}` in the browser command); otherwise a second launch hands off to the running Chromium and exits.
  - Plex: Chromium kiosk, app_id `lumen-plex`, ~1.7 GB, F1 opens Chrome help (needs an rc.xml swallow).
  - Steam Link: `steamlink` (Xwayland), app_id `SteamLink`; launcher script exits at once, real app `~/.local/share/SteamLink/bin/shell` reparented to PID 1.
  - Kodi: `kodi --windowing=wayland`, app_id `Kodi`, real app `kodi.bin`, close takes 4.5 s.

- (2026-10-09) Audio: PipeWire (`pipewire pipewire-pulse pipewire-alsa wireplumber`, user services) mixes for the HDMI output (vc4-hdmi-0, volume 100%). Without it Chromium's second audio stream got "Device or resource busy" and played silently (the Pi 5 HDMI PCM has no mixing). Kodi/Steam Link with pipewire-alsa still to be checked.
- (2026-10-09) Startup race: if Lumen starts before HDMI-A-1 is ready, `start-lumen.sh` can't set the mode and Lumen's window comes up tiny, top-left, not fullscreen (labwc "invalid output set for view"). Fixed in 2.1: `start-lumen.sh` waits up to ~5 s for the output, and Lumen asks for fullscreen on its own window if it comes up windowed.

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
  - Phase 2 (Job 2.1) done: registry, lifecycle, windows, icons and the Home gate are wired into main (`src/session.rs` holds the glue). Smoke test passed on the Pi (launch, F1 back, same PID on re-select, exclusive Steam Link ends Jellyfin). Phase 3: review/simplify/tests/docs. Phase 4: gate G1–G6 on the Pi, then at the TV.
  - Every app runs as a child of Lumen inside labwc (no systemd units). While an app is in front, Lumen acts on Home alone (keyboard).
  - **Review 3.1 (2026-10-10, diff 2f04336..37446a4): Patch and ship.** Lifecycle, gate, seeding and icons hold up; forbidden dirs untouched; Mac build gated.
    - **Critical** FIXED 7a8870c: planner `retiring` record (windows never rebound, PIDs ignored by the poll); new run starts at once, no Spawn deferral. `apps.rs` `process_poll` + `session.rs`: Home on Steam Link, then re-select within ~5 s (old `shell` still in its 2 s/5 s kill window): the poll finds the old shell → `seen`; it dies → `finish` forgets the new launch → its window binds to nothing → gate closed, Home a no-op (fg=Lumen). Kodi variant: a `Changed` from the dying old `Kodi` window binds by app_id to the new run, then its `Closed` terminates the new run. Fix: planner keeps a `retiring` set (id + window ids) per Terminate; never bind a retired window id (ids are monotonic in toplevel.rs), and defer `Spawn` of a retiring id until its terminate finishes (or have the poll ignore the PIDs `collect_targets` returned).
    - **Required** FIXED 7a8870c: `Lifecycle::home(lumen_activated)`. `session::home()`: no safety net when Lumen's window isn't activated but the planner says fg=Lumen (an unbound window in front, a launch still unbound, the race above): every other key is dropped and Home does nothing. Fix: `Lifecycle::home(lumen_activated)`; if not activated, always emit `ActivateLumen` (and Terminate a close-on-exit app still launching).
    - **Required** FIXED 7a8870c: `bg` dropped from `App`, `bake::Tint` kept in Rust (`Home.tints`). `main.rs` `tile_brush`: `slint::private_unstable_api` has no semver promise and `slint = "1.18"` takes 1.19 on `cargo update`. Slint 1.18 exports no public `LinearGradientBrush`/`GradientStop` constructor. Slint never draws `bg` (it's only bake input), so drop `bg` from `App`, keep each tile's `Vec<Color>` in Rust beside its icon, and add `bake::Ramp::from_stops`; this removes the import and the brush round trip. Building it in .slint is worse: 2–3 stops need branches, and Rust still has to read the stops back.
    - **Nit** FIXED 7a8870c: 1 s timer retry for 10 s. `session.rs` `MAX_FULLSCREEN_REQUESTS`: the cap counts Lumen window events, not time, so 5 early events while there's no output use it up. Belt-and-braces now that `start-lumen.sh` waits; a 10 s timer retry would be sturdier. `select()` before `set_xdg_app_id` is correct in 1.18: without a platform it returns `NoPlatform`.
    - **Nit** FIXED 7a8870c: `libc::kill`, terminate cfg(linux); file split skipped (optional). `apps.rs` `extern "C" fn kill` + magic SIGTERM/SIGKILL: use `libc::kill`/`libc::SIGTERM`. libc is Linux-only and apps.rs builds on the Mac, so `#[cfg(target_os = "linux")]` the process-control half (the Design calls it Linux). That also splits the 1037-line file into `apps/lifecycle.rs` + `apps/process.rs`.
    - **Nit** SKIPPED (no brand artwork in the repo). `config/apps/jellyfin.toml`: icon `org.jellyfin.JellyfinDesktop` comes from the dropped Flatpak; uninstalling it silently turns the tile into "J". Ship `icons/jellyfin.svg` instead.
    - **FYI** bench: `pass` judges only the 30 s motion phase (`fps ≥ 59 && slow < 2`), which is the pre-registered gate, so 4K `pass:true` is honest. Idle (3 s of forced back-to-back redraws) at 9.8% slow is a headroom signal, not a gate item. Report idle next to motion in G7, and re-bench without icons if it's worse than before.

## Known issues
- After `systemctl stop lumen-test`, its ExecStopPost starts Kodi: wait until kodi is active before starting a test unit again.
- `ssh pitv` uses the IP (192.168.1.60), since `pitv.local` (mDNS) doesn't resolve over the VPN.
- `/dev/uinput` is writable by the input group: `cargo run --release --example fake-pad [secs]` on the Pi tests the input layer without a controller.
- Perf lines log under `labwc[...]`, so grep the whole journal, not `-u lumen-test`.
- Don't run wf-recorder on the Pi: its frames come out garbled and it froze the TV output.
- History: the WebKit focus-change flash/snap (compositor drew pre-animation values for ~2 frames) was patched with "hold" animations (95dc6f9); gone since the Slint port.
- Benchmark: put `LUMEN_BENCH=1` in `~/.config/lumen-tv/env` (runs on every start; remove it after), or press B on the home screen.

## Blockers
- (none)
- Bench is thermally sensitive: the Pi 5 has no fan and sat at 75-82 C after builds (4K fell to 50-55 fps until it cooled; a cool 4K run passed at 60 fps). Let it cool before benching.

## Next Steps
1. M3 Job 0.3: client spike in the Lumen session (launch line, app_id, PID model, close, fullscreen, audio) for Jellyfin, Plex-in-Chromium, Steam Link, Kodi.
2. M3 Phase 1 builders.
3. Parked for M5 input polish (don't work on these before then): CEC Back/Home (`evtest /dev/input/event5` + `sudo cec-ctl -d /dev/cec0 -M`, try `--vendor-id 0x00e091`), remote Home fallback, PS5 controller over USB/BT.

## Recently Completed
- 4K bake merge (2026-10-08): `exp/baked-hero` merged into main (0abbe64). 4K60 59.7 fps / 0.4% > 25 ms / p95 17.4 ms (was 27.8 fps); 1080p60 60.0 / 0.1% / p95 17.1. M2 input kept; HDMI remote verified at 4K. Default stays 1080p. Branch, worktree and `~/lumen-bake` on the Pi removed.
- M1.5 Slint port (2026-10-08): gate passed (60.0 fps, 0.1% > 25 ms, Laith: no artifacts). Spike branch tagged `spike-slint-2026-10-08`, worktree and Pi test files removed.
- Slint spike (2026-10-08): passed its gate at 1080p (femtovg 60.0 fps, 0.1% slow, no flash); 4K too slow with live effects (fixed later by baking). Laith chose Slint. Report: `git show spike-slint-2026-10-08:spike-slint/REPORT.md`.
