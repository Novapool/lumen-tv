# Milestones — Lumen (lumen-tv)

Spec: `BRIEF.md`. Each milestone stops for Laith to test on the TV before the next starts.

## Scope changes
- **2026-10-08, UI stack (Laith):** Slint 1.18 (femtovg renderer, winit backend under labwc) replaces Tauri 2 + Svelte + WebKitGTK, which BRIEF §2 names. Reason: the Slint spike (tag `spike-slint-2026-10-08`, `spike-slint/REPORT.md`) hit 60.0 fps / 0.1% slow frames at 1080p60 with no focus flash, vs WebKit's 56.6 fps / 9.8%. All Rust (input, registry, store, platform traits) is unaffected; the BRIEF §3 "Tauri events / commands" bridge becomes Slint callbacks + `slint::invoke_from_event_loop`. Output stays 1080p60 (live gradients and shadows were too slow at 4K on the Pi 5 GPU with every toolkit; see the 4K entry below).
- **2026-10-08, 4K output (Laith):** 4K60 works after baking the hero backdrops, tile faces and focus shadow into images on the CPU (`src/bake.rs`, merged from `exp/baked-hero`), so the fill-rate-bound GPU draws one backdrop layer. Bench on main (0abbe64): 4K60 59.7 fps, 0.4% > 25 ms, p95 17.4 ms (was 27.8 fps); 1080p60 60.0 fps, 0.1%, p95 17.1 ms. Default stays 1080p60 for now; `LUMEN_MODE=3840x2160@60` selects 4K.
- **2026-10-08, M2 gate cut (Laith):** M2 closes on the TV remote alone. The real controller (PS5 DualSense over USB/BT; `playstation.toml` already maps it) and keyboard checks move to M5 polish.
- **2026-10-09, input layer docked (Laith):** the keyboard is the control for development; the input layer stays as built in M2 and gets no new work. Parked until an "Input polish" pass in M5: CEC Back/Home (never reached Lumen; likely needs an LG vendor-ID claim), a Home fallback gesture for the remote, controller (PS5 DualSense) and the rest of the M2 gate. M3 keeps only what launching needs: while an app is in front, Lumen acts on Home alone (keyboard Home/F1 per `keyboard.toml`).
- **2026-10-09, icons + debug Settings earlier (Laith):** app icons are drawn on tiles in M3 (BRIEF's `icon` field), not M5. A Settings/diagnostics screen moves from M5 into a new M3.5, to make debugging on the TV easier.
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

## M2 — Input layer ✅ (2026-10-08, remote only)
evdev reader in Rust (non-grabbing), hotplug via inotify on `/dev/input`, per-device TOML mappings (`config/input/*.toml`), normalized events to the UI (reader thread → `slint::invoke_from_event_loop` → UI callbacks), hold-to-repeat with acceleration, stick deadzone, long-press Back → Home. Spatial navigation in the UI. CEC remote buttons verified with `cec-ctl` + `evtest`; `cec-remote.toml` generated from what the TV actually sends.
**Gate:** TV remote, Xbox controller and keyboard each navigate the tile row and fire Select/Back/Menu/Home; unplugging and replugging (or BT disconnect/reconnect) the controller works without restarting Lumen; a resting stick produces zero events over 60 s.
**Result:** passed for the TV remote only (Laith's call, see scope change). On the Pi at 4K the log shows Left/Right/Select from `vc4-hdmi-0`; Back and Home never appeared in `[input]`, so the TV may not forward them over CEC (open, M3 needs a working Home). Controller paths verified only with `examples/fake-pad.rs` (hotplug, all buttons, 60 s drift = 0).

## M3 — Registry + launching + icons ✅ (2026-10-13)
TOML app entries in `~/.config/lumen-tv/apps/` (each with an `icon`, drawn on its tile), launch/raise/close via a `platform::Windows` trait (labwc + wlr-foreign-toplevel), keep-alive / close-on-exit / exclusive policies, auto-return when an app exits, Home works while another app is focused. Clients (installed 2026-10-09): Jellyfin Desktop (Flathub, user install), Plex web in Chromium kiosk (Plex HTPC is x86-only), Steam Link under Xwayland, Kodi on Wayland. Home comes from the keyboard (input layer docked).
**Gate:** Home returns to Lumen from Jellyfin and from Steam Link; reselecting Jellyfin resumes the same process (same PID) in < 1 s; launching Steam Link kills running keep-alive apps; Home in Steam Link ends its process; killing an app from SSH brings Lumen to the front.
**Measured by:** G1–G6 in the M3 plan: Home and resume < 1000 ms over 5/5 trials with the same PID, exclusive kill within 5 s, auto-return within 2000 ms of a kill, and input aimed at an app never drives Lumen. Plus: every shipped app shows its icon on its tile on the TV.
**Result:** pass. Automated on the Pi (2026-10-10): Home 2–7 ms, reselect same PID 5/5, exclusive kill 1.2 s, kill→Lumen 3–41 ms, input guard OK; bench 60 fps / 0.1% slow at 1080p and 4K. At the TV (2026-10-13): icons sharp (Plex keeps its glyph, no brand artwork ships); Jellyfin Home → reselect returns to the same paused video; Steam Link (Forza) smooth with sound, F1 ends it; SSH kill of Jellyfin → Lumen in front in 23 ms. Client changes: Jellyfin = web client in Chromium kiosk (the Flathub app had no GPU and no video). Scope additions: labwc swallows F1/Super_L (Chromium help page); Home pauses keep-alive media via MPRIS and reselect does not auto-resume (Laith); Steam Link's menu→stream window swap gets a 10 s windowless grace.

## M3.5 — Settings / diagnostics screen (draft, from M5)
A Settings tile opens a screen for debugging on the TV: output mode and live fps, input devices with the last action each sent, registry entries with any parse errors, running apps with PIDs and window state, the tail of Lumen's log, and toggles for input debug and the bench. Reached with the keyboard.
**Gate:** (draft, Laith to confirm) every panel shows live data on the TV; a broken app TOML shows its error there instead of only in the journal; opening Settings keeps the home screen at ≥ 59 fps / < 2% slow frames.

## M4 — Store
Fetch `manifest.json` from a configurable URL, Store screen, details screen showing the exact install/uninstall commands, confirm-before-run, streamed progress + error states, registry entry written on success, home grid updates live.
**Gate:** installing an app from the store makes its tile appear on Home without restarting Lumen; a deliberately failing install step shows the error on screen and writes no registry entry; uninstall removes both the package and the tile.

## M5 — Boot session + polish
Auto-login → labwc → Lumen only (no panel/desktop), install script (packages, input group/udev, units), Settings additions (input mappings, manifest URL, about), input polish (parked M2 items: CEC Back/Home, remote Home fallback, controller over USB/BT), visual polish pass, README (setup, add an app, add a controller, change manifest).
**Gate:** cold power-on lands on Lumen with no desktop or console text visible after the boot splash; every "Done means" item in `BRIEF.md` §11 checked off on the TV.
