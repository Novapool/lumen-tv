# Slint spike: report (2026-10-08)

Question: does Slint beat Tauri 2 + Svelte 5 + WebKitGTK 2.54 on the Pi 5?
Spike = the M1 home screen rebuilt in Slint 1.18.1 (`ui/home.slint`, `src/main.rs`),
same 30 s bench as `ui/src/lib/perf.js`. Run on pitv under labwc via
`session/lumen-slint-test.service` (env in `~/.config/lumen-slint/env`).

## Gate (fixed before measuring)
1080p: >= 59 fps and < 2% frames > 25 ms, OR 4K: >= 55 fps; no focus artifacts on the TV.

## Results (motion phase, 30 s, Right every 150 ms)

| Run | fps | > 25 ms | p95 | worst |
|---|---|---|---|---|
| WebKit 1080p60 (baseline, main @ 95dc6f9) | 56.6 | 9.8% | | |
| Slint femtovg 1080p60, shadow scales with tile | 56.5 | 5.2% | 25.6 | 40.5 |
| Slint femtovg 1080p60, no shadow | 60.0 | 0.1% | 17.0 | 33.4 |
| **Slint femtovg 1080p60, fixed fading shadow** | **60.0** | **0.1%** | 17.6 | 34.3 |
| Slint Skia (OpenGL) 1080p60, fixed shadow | 55.2 | 4.4% | 24.8 | 40.6 |
| WebKit 4K (baseline) | 9.4 | | | |
| Slint femtovg 4K60 | 27.6 | 98.8% | 51.9 | 70.9 |
| Slint femtovg 4K30 | 27.3 | 99% | 66.7 | 71.7 |
| Slint Skia (OpenGL) 4K60 | 14.1 | 84.3% | 146.8 | 179.9 |

Idle (static scene force-redrawn every frame): 60 fps at 1080p for both renderers,
31 fps (femtovg) / 24 fps (Skia) at 4K, so 4K is GPU fill-rate bound, not animation bound.

**Gate: PASS at 1080p (femtovg, fixed shadow). FAIL at 4K.**
Laith on the TV: "looks smooth, no flashes"; holding an arrow key was a little choppy
with the scaling shadow, "mostly gone" without it. 4K "choppy", 1080p "way smoother".
Skia via its default wgpu/Vulkan surface also drew choppily (no numbers: that surface
never fires Slint's AfterRendering notifier).

## Findings
- No stale-value flash/snap: Slint animates properties on the main thread and draws
  each frame itself, so there is no compositor to go out of sync (no "hold" hacks needed).
- Drop shadows that change size re-render their blur every frame (cost ~4 fps + 5% slow
  frames). Keep shadow geometry fixed and fade it. Slint draws no shadow for an unfilled
  rectangle, so the shadow box needs a fill (hidden behind the face).
- femtovg beats Skia on the Pi 5 at both resolutions.
- Skia needs `renderer-skia-opengl`; prebuilt skia binaries exist for aarch64, no apt needed.
- Slint radial gradients are circles only (the hero fade is approximated).
- Not tested: linuxkms backend on tty1 without labwc (needs apt libinput-dev libudev-dev
  libgbm-dev libdrm-dev). Unlikely to fix 4K: idle redraw is already fill-bound.

## Migration cost
- Rewrite: the Svelte UI (~600 lines on main today: App, Tile, perf, CSS) in .slint,
  and every future screen (Store, details/menu, Settings) in Slint instead of Svelte.
  The spike's home screen is ~510 lines including the bench.
- Carries over: all Rust. M2's evdev input layer, M3's registry/launcher/platform traits,
  M4's manifest fetch and install runner. Only the UI bridge changes (Tauri events /
  commands -> Slint callbacks + `invoke_from_event_loop`). Builds natively on the Pi as now.
- Drops: Tauri, Node/Vite, WebKitGTK as a dependency of the launcher itself. Fallback web
  clients (Jellyfin/Plex `fallback_url` kiosk window) then need a separate browser
  (e.g. Cog/WPE or Chromium kiosk) instead of a Tauri webview window.
- Scope: BRIEF.md names Tauri 2 + Svelte; switching is Laith's call, recorded in MILESTONES.
- License (Slint 1.18.1: GPL-3.0-only OR Royalty-free 2.0 OR paid): the Royalty-free
  license covers desktop/mobile/web apps and **excludes embedded systems** ("a computer
  system designed to perform a specific task"), which a boot-to-launcher TV box arguably
  is. Safe route: GPLv3. Lumen has no license yet; the repo is public, so Lumen would be
  published as GPLv3 (fine for a personal open-source project; would matter only if sold).
