# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-06

## Where we are
Project just started. The brief is saved in `BRIEF.md` and the milestones in `MILESTONES.md`. No code yet. Waiting for Laith to approve the plan and answer the open questions before M1 begins. Kodi is still the live front end on pitv and stays that way until M5.

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

## Open questions (need Laith)
1. Kodi currently provides Games (RetroPlayer), YouTube and Screen Mirror. "Replaces Kodi entirely" would drop them. Proposal: keep Kodi as a Lumen app tile (close-on-exit, exclusive) until replacements exist.
2. New apt packages on the Pi for M1: labwc, Rust toolchain (rustup), Node, WebKitGTK 4.1 + GTK dev libs. OK to install?
3. Store manifest "Git repo": create a GitHub repo now (e.g. lumen-tv, public, so a raw URL works), or use a local file until M4?
4. How much RAM does this Pi 5 have? (Affects native Rust build time and the keep-alive budget.)
5. M1 perf fallback if WebKitGTK can't hold 60 fps: (a) run at 1080p instead of 4K, (b) WebKit flags / reduced effects, (c) switch the webview to WPE/Cog, (d) a native UI (Slint). Decide only if the M1 gate fails.

## Current Work
- (none — waiting for go-ahead)

## Blockers
- Plan approval + open questions above.

## Next Steps
1. Laith approves plan and answers open questions.
2. M1: install build deps on the Pi, scaffold Tauri + Svelte, test unit, frame-time overlay, measure on the TV.

## Recently Completed
- Project folder, brief, milestones and tracking docs created (2026-10-06).
