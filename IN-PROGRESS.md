# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-08

## Where we are
M1.5 done (2026-10-08): Lumen's UI is now Slint (femtovg) on main, 60.0 fps / 0.1% slow at 1080p60 on the TV, no focus artifacts. Tauri, Svelte and WebKitGTK are gone. Now on **M2, input layer**: built and tested with a virtual controller; waiting on real remote/controller checks at the TV. Kodi still boots by default; Lumen runs from `lumen-test.service`.

## Active Plan
Build M2 → M5 in order (see MILESTONES.md), stopping after each one for a TV test.

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
- (2026-10-06) Kodi stays as a Lumen tile (close-on-exit, exclusive) for Games/YouTube/Mirror until replacements exist.
- (2026-10-06) Rust 1.99 via rustup (`~/.cargo/bin`) on the Pi and the Mac (not on the Mac's PATH). apt packages need Laith's sudo.
- (2026-10-06) Store manifest lives in this repo: github.com/Novapool/lumen-tv, `store/manifest.json` (raw URL once pushed).
- (2026-10-06) Pi 5 has 8 GB RAM. laith is already in the `input` group. TV native mode is 3840x2160; Lumen sets 1080p60 with `wlr-randr` (4K fails with every toolkit).

## Slint rules (from the spike)
- Never animate a drop shadow's size or blur: the blur re-renders every frame (56.5 fps, 5.2% slow). Keep the shadow box fixed and fade it with `opacity`. Slint draws no shadow for an unfilled Rectangle, so fill it and hide it behind the face.
- Slint redraws only on change: frame stats count only frames while something is animating.
- No "hold" animation workaround needed (that was a WebKit compositor bug).
- Radial gradients are circles only.

## Current Work
- **M2 — Input layer** (2026-10-08). Done: `src/input/` (evdev readers, inotify hotplug, mapping files, repeat 400 ms then 160 → 60 ms, stick deadzone 0.4 with hysteresis, long-press Back 0.8 s → Home), `config/input/*.toml` (keyboard, cec-remote, gamepad, xbox, playstation; user overrides by file name in `~/.config/lumen-tv/input/`), `platform::Tv` (Lumen registers with the TV over CEC as Playback "Lumen" at startup and becomes the active input, like Kodi). Window key handling is off on the Pi (`key-nav`); the Mac build still navigates with its own keys.
  - Verified on pitv with `examples/fake-pad.rs` (virtual Xbox pad via uinput): hotplug, every mapped button, long-press Home, accelerating repeat, 60 s drift → 0 events, unplug. 15 unit tests pass on the Pi.
  - Left for the gate (needs Laith at the TV): real CEC remote presses (`cec-remote.toml` is a draft from the kernel keymap), a real Xbox controller over USB and Bluetooth incl. disconnect/reconnect, keyboard. `~/.config/lumen-tv/env` has `LUMEN_INPUT_DEBUG=1`: each action shows at the bottom of the screen and logs `[input] <Action> from "<device>"`.
  - Open: spatial navigation for multi-row screens is deferred until a screen has more than one row (M4 Store); the home row only uses Left/Right (Home jumps to the first tile).

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

## Next Steps
1. At the TV: `evtest /dev/input/event5` while pressing every remote button (Up/Down/Left/Right, OK, Back, Home, Menu/Settings, Play/Pause, hold Right); trim `cec-remote.toml` to what arrives. If a button never shows up, check whether the TV sends it with `sudo cec-ctl -d /dev/cec0 -M` (monitor needs root).
2. Pair/plug an Xbox controller, run the M2 gate (MILESTONES), then remove `LUMEN_INPUT_DEBUG` from the env file.

## Recently Completed
- M1.5 Slint port (2026-10-08): gate passed (60.0 fps, 0.1% > 25 ms, Laith: no artifacts). Spike branch tagged `spike-slint-2026-10-08`, worktree and Pi test files removed.
- Slint spike (2026-10-08): passed its gate at 1080p (femtovg 60.0 fps, 0.1% slow, no flash); 4K fails everywhere. Laith chose Slint. Report: `git show spike-slint-2026-10-08:spike-slint/REPORT.md`.
- M1 skeleton + perf (2026-10-06): passed revised gate at 1080p (55.9 fps); 4K not viable in WebKit. Details in MILESTONES.md.
