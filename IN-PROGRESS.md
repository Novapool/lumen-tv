# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-06

## Where we are
M1 done: Lumen runs fullscreen under labwc at 1080p60 from `lumen-test.service` (Kodi still boots by default). Smooth enough per Laith, with a known focus-change artifact under separate investigation. Now on M2 (input layer).

## Active Plan
Build M1 → M5 in order (see MILESTONES.md), stopping after each one for a TV test.

Planned layout:
```
lumen-tv/
  src-tauri/src/
    main.rs, lib.rs        Tauri setup, commands, event bridge
    input/                 evdev reader, hotplug, mapping loader, repeat/deadzone
    registry.rs            ~/.config/lumen-tv/apps/*.toml
    apps.rs                launch / track / close / lifecycle policies
    store.rs               manifest fetch, install/uninstall runner
    platform/mod.rs        traits: Windows, Display, Installer
    platform/pi.rs         labwc + foreign-toplevel, apt/flatpak, HDMI mode
  ui/                      Svelte + Vite
    src/lib/spatial.ts     spatial focus navigation
    src/screens/           Home, Store, AppDetails, Settings
    src/components/Tile.svelte
  config/input/            keyboard.toml, xbox.toml, playstation.toml, cec-remote.toml
  store/manifest.json      sample: Jellyfin, Plex, Steam Link
  session/                 labwc rc.xml + autostart, systemd/getty units, install.sh
  README.md
```

Decisions so far:
- Registry in `~/.config/lumen-tv/apps/` (XDG config dir: user-editable, survives reinstalls, no root needed). Mapping defaults ship in the app; user overrides go in `~/.config/lumen-tv/input/`.
- Build Rust natively on the Pi (simplest; no cross-compile toolchain for WebKitGTK).
- Window control through labwc's wlr-foreign-toplevel-management protocol (activate/close), behind `platform::Windows`.

## Settled (2026-10-06)
- Kodi stays as a Lumen tile (close-on-exit, exclusive) for Games/YouTube/Mirror until replacements exist.
- Build deps approved. Rust 1.99 installed via rustup (`~/.cargo/bin`). apt packages need Laith's sudo (see Next Steps).
- Store manifest lives in this repo: github.com/Novapool/lumen-tv, `store/manifest.json` (raw URL once pushed).
- Pi 5 has 8 GB RAM. laith is already in the `input` group (evdev readable). TV mode is 3840x2160.
- Perf fallback order if M1 fails: 1080p output → fewer effects → WPE/Cog → Slint. Laith chose 1080p + cheaper effects (2026-10-06); `wlr-randr` installed.

## Current Work
- **M2 — Input layer** (starting 2026-10-06). See MILESTONES.md M2.

## Known issues
- Focus change on the Pi still shows a "drawn wrong, then snaps into place" artifact (Laith, 2026-10-06). Separate investigation handed off; candidates: WebKit re-rasterizing scaled layers at the end of a transform transition, subpixel snapping, layer promotion. ~1 dropped frame per focus change is also open (M5 polish).
- M1 perf details: MILESTONES.md M1. Perf lines log under `labwc[...]`, so grep the whole journal, not `-u lumen-test`.

## Blockers
- (none)

## Next Steps
1. M2: check what the CEC remote and controller actually send (`cec-ctl`, `evtest`), then build the evdev reader + mappings.

## Recently Completed
- M1 skeleton + perf (2026-10-06): passes revised gate at 1080p (55.9 fps, looks smooth); 4K not viable in WebKit on Pi 5. Details in MILESTONES.md.
- Build deps installed on pitv (2026-10-06): needed `apt update` first (stale index 404'd on webkit2gtk security update).
- Project folder, brief, milestones and tracking docs created (2026-10-06).
