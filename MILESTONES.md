# Milestones — Lumen (lumen-tv)

Spec: `BRIEF.md`. Each milestone stops for Laith to test on the TV before the next starts.

## M1 — Skeleton + perf check ✅ (2026-10-06)
Tauri 2 + Svelte 5/Vite, fullscreen under labwc via `lumen-test.service`, static 12-tile row, frame-time overlay + 30 s bench.
**Gate (revised 2026-10-06 by Laith):** ≥ 55 fps average during the 30 s hold-Right bench at 1920x1080 output, and Laith judges it smooth on the TV. Original gate (≥ 55 fps and < 5% frames > 25 ms at native 4K) failed.
**Result:** 4K = 9.4 fps (GPU fill rate; CPU rendering and DMA-BUF-off are worse). 1080p60 output (wlr-randr in `session/start-lumen.sh`) = 55.9 fps, 16% slow frames, ~1 dropped frame per focus change even with all effects off (fixed WebKit per-change cost). Removing the tilt spring-back and title slide fixed most visible glitching; pre-painting stacked backdrops made it worse (12 full-width layers). Open: residual "redraw then snap" artifact on focus change (see IN-PROGRESS Known issues).

## M2 — Input layer
evdev reader in Rust (non-grabbing), hotplug via inotify on `/dev/input`, per-device TOML mappings (`config/input/*.toml`), normalized events to the UI, hold-to-repeat with acceleration, stick deadzone, long-press Back → Home. Spatial navigation in the UI. CEC remote buttons verified with `cec-ctl` + `evtest`; `cec-remote.toml` generated from what the TV actually sends.
**Gate:** TV remote, Xbox controller and keyboard each navigate the tile row and fire Select/Back/Menu/Home; unplugging and replugging (or BT disconnect/reconnect) the controller works without restarting Lumen; a resting stick produces zero events over 60 s.

## M3 — Registry + launching
TOML app entries in `~/.config/lumen-tv/apps/`, launch/raise/close via a `platform::Windows` trait (labwc + wlr-foreign-toplevel), keep-alive / close-on-exit / exclusive policies, auto-return when an app exits, Home works while another app is focused. Jellyfin / Plex / Steam Link client choice verified on the Pi.
**Gate:** Home returns to Lumen from Jellyfin and from Steam Link; reselecting Jellyfin resumes the same process (same PID) in < 1 s; launching Steam Link kills running keep-alive apps; Home in Steam Link ends its process; killing an app from SSH brings Lumen to the front.

## M4 — Store
Fetch `manifest.json` from a configurable URL, Store screen, details screen showing the exact install/uninstall commands, confirm-before-run, streamed progress + error states, registry entry written on success, home grid updates live.
**Gate:** installing an app from the store makes its tile appear on Home without restarting Lumen; a deliberately failing install step shows the error on screen and writes no registry entry; uninstall removes both the package and the tile.

## M5 — Boot session + polish
Auto-login → labwc → Lumen only (no panel/desktop), install script (packages, input group/udev, units), Settings screen (input mappings, manifest URL, about), visual polish pass, README (setup, add an app, add a controller, change manifest).
**Gate:** cold power-on lands on Lumen with no desktop or console text visible after the boot splash; every "Done means" item in `BRIEF.md` §11 checked off on the TV.
