# lumen-tv

Lumen: an Apple TV-style launcher for pitv (Tauri 2 + Svelte, Rust evdev input, labwc session). It is meant to replace Kodi. Pi access, network and box rules are in `../CLAUDE.md`.

## Docs (read when)
- `BRIEF.md`: Laith's original spec. Read before planning or changing scope. Never edit it.
- `MILESTONES.md`: milestone specs + Gate lines. Read before starting or closing a milestone.
- `IN-PROGRESS.md`: current state, open questions, next steps. Read before any work.

## Rules
- Build milestone by milestone and stop after each one for Laith to test on the TV.
- Kodi stays the boot default until M5 passes. Test Lumen through a unit that `Conflicts=kodi.service`.
- All Pi-specific code lives behind traits in `src-tauri/src/platform/`.
- Never run store install commands without showing them first.
