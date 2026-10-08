# Milestones — Lumen (lumen-tv)

Spec: `BRIEF.md`. Each milestone stops for Laith to test on the TV before the next starts.

## Scope changes
- **2026-10-08, UI stack (Laith):** Slint 1.18 (femtovg renderer, winit backend under labwc) replaces Tauri 2 + Svelte + WebKitGTK, which BRIEF §2 names. Reason: the Slint spike (tag `spike-slint-2026-10-08`, `spike-slint/REPORT.md`) hit 60.0 fps / 0.1% slow frames at 1080p60 with no focus flash, vs WebKit's 56.6 fps / 9.8%. All Rust (input, registry, store, platform traits) is unaffected; the BRIEF §3 "Tauri events / commands" bridge becomes Slint callbacks + `slint::invoke_from_event_loop`. Output stays 1080p60 (4K fails with every toolkit on the Pi 5 GPU).
- **2026-10-08, license (Laith):** Lumen is published under GPLv3, since Slint's royalty-free license excludes embedded systems.
- **2026-10-08, fallback web clients:** `fallback_url` (Jellyfin/Plex web in kiosk mode) can no longer open in a Tauri webview window. It needs a separate kiosk browser (Cog/WPE or Chromium); the choice is made in M3, and only if no native client works.

## M1 — Skeleton + perf check ✅ (2026-10-06)
Tauri 2 + Svelte 5/Vite, fullscreen under labwc via `lumen-test.service`, static 12-tile row, frame-time overlay + 30 s bench.
**Gate (revised 2026-10-06 by Laith):** ≥ 55 fps average during the 30 s hold-Right bench at 1920x1080 output, and Laith judges it smooth on the TV. Original gate (≥ 55 fps and < 5% frames > 25 ms at native 4K) failed.
**Result:** 4K = 9.4 fps (GPU fill rate; CPU rendering and DMA-BUF-off are worse). 1080p60 output (wlr-randr in `session/start-lumen.sh`) = 55.9 fps, 16% slow frames, ~1 dropped frame per focus change even with all effects off (fixed WebKit per-change cost). Removing the tilt spring-back and title slide fixed most visible glitching; pre-painting stacked backdrops made it worse (12 full-width layers). The focus-change flash/snap was fixed with "hold" animations (56.6 fps, 9.8% slow).
**Slint spike (2026-10-08, same bench):** femtovg 1080p60 = 60.0 fps, 0.1% > 25 ms, p95 17.6 ms, no flash/snap ("looks smooth, no flashes"). Skia (OpenGL) 1080p60 = 55.2 fps / 4.4%. 4K: femtovg 27.6 fps, Skia 14.1, WebKit 9.4 (fill-rate bound; idle redraw alone is 31 fps). Led to the stack change above.

## M1.5 — Port the UI to Slint ✅ (2026-10-08)
One cargo crate at the repo root (`src/` + `ui/*.slint`) replaces `src-tauri/` + the Svelte `ui/`. The M1 home screen comes over from the spike (femtovg, fixed fading shadow) with the `LUMEN_BENCH=1` 30 s bench. `scripts/pi-build.sh` loses the npm step; `session/start-lumen.sh` points at the new binary; `lumen-test.service` is unchanged. GPLv3 `LICENSE` added. Tauri, Node/Vite and WebKitGTK are no longer build dependencies.
**Gate:** the Slint home screen built from main, run from `lumen-test.service` at 1080p60, averages ≥ 59 fps with < 2% of frames > 25 ms in the 30 s bench, and Laith sees no focus artifacts on the TV.
**Result:** 60.0 fps, 0.1% > 25 ms, p95 17.6 ms, worst 33.9 ms; idle 60.3 fps (same as the spike). Laith: TV looks good, no artifacts. Pass.

## M2 — Input layer
evdev reader in Rust (non-grabbing), hotplug via inotify on `/dev/input`, per-device TOML mappings (`config/input/*.toml`), normalized events to the UI (reader thread → `slint::invoke_from_event_loop` → UI callbacks), hold-to-repeat with acceleration, stick deadzone, long-press Back → Home. Spatial navigation in the UI. CEC remote buttons verified with `cec-ctl` + `evtest`; `cec-remote.toml` generated from what the TV actually sends.
**Gate:** TV remote, Xbox controller and keyboard each navigate the tile row and fire Select/Back/Menu/Home; unplugging and replugging (or BT disconnect/reconnect) the controller works without restarting Lumen; a resting stick produces zero events over 60 s.

## M3 — Registry + launching
TOML app entries in `~/.config/lumen-tv/apps/`, launch/raise/close via a `platform::Windows` trait (labwc + wlr-foreign-toplevel), keep-alive / close-on-exit / exclusive policies, auto-return when an app exits, Home works while another app is focused. Jellyfin / Plex / Steam Link client choice verified on the Pi; if a web client is needed, pick the kiosk browser for `fallback_url` (Cog/WPE or Chromium).
**Gate:** Home returns to Lumen from Jellyfin and from Steam Link; reselecting Jellyfin resumes the same process (same PID) in < 1 s; launching Steam Link kills running keep-alive apps; Home in Steam Link ends its process; killing an app from SSH brings Lumen to the front.

## M4 — Store
Fetch `manifest.json` from a configurable URL, Store screen, details screen showing the exact install/uninstall commands, confirm-before-run, streamed progress + error states, registry entry written on success, home grid updates live.
**Gate:** installing an app from the store makes its tile appear on Home without restarting Lumen; a deliberately failing install step shows the error on screen and writes no registry entry; uninstall removes both the package and the tile.

## M5 — Boot session + polish
Auto-login → labwc → Lumen only (no panel/desktop), install script (packages, input group/udev, units), Settings screen (input mappings, manifest URL, about), visual polish pass, README (setup, add an app, add a controller, change manifest).
**Gate:** cold power-on lands on Lumen with no desktop or console text visible after the boot splash; every "Done means" item in `BRIEF.md` §11 checked off on the TV.
