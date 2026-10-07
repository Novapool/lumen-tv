# Milestones — Lumen (lumen-tv)

Spec: `BRIEF.md`. Each milestone stops for Laith to test on the TV before the next starts.

## M1 — Skeleton + perf check
Tauri 2 + Svelte/Vite app, fullscreen under labwc on the Pi, static tile row with focus animation (scale, shadow, tilt, sheen) driven by keyboard. Built natively on the Pi. Runs from a test unit that `Conflicts=kodi.service`, so Kodi stays the default until M5.
**Gate:** on the TV at the Pi's native output mode, holding Right across 10+ tiles for 30 s keeps ≥ 55 fps average and < 5% frames over 25 ms, measured by an in-app `requestAnimationFrame` frame-time overlay. Fail → stop and propose options (see IN-PROGRESS Open questions).

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
