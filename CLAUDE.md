# lumen-tv

Lumen: an Apple TV-style launcher for pitv (Rust + Slint UI, evdev input, labwc session), meant to replace Kodi. GPLv3. Pi access, network and box rules are in `../CLAUDE.md`.

## Docs (read when)
- `BRIEF.md`: Laith's original spec. Read before planning or changing scope. Never edit it (scope changes go in MILESTONES.md).
- `MILESTONES.md`: scope changes, milestone specs + Gate lines. Read before starting or closing a milestone.
- `IN-PROGRESS.md`: current state, Slint rules, open questions, next steps. Read before any work.

## Layout
- `src/`: Rust (main + Slint callbacks, input, registry, apps, store, `platform/`). `ui/`: `.slint` files, compiled by `build.rs`.
- `session/`: labwc config, `start-lumen.sh`, `lumen-test.service`, sudoers. `scripts/pi-build.sh`: sync to pitv and build there.

## Rules
- Build milestone by milestone and stop after each one for Laith to test on the TV.
- Kodi stays the boot default until M5 passes. Test Lumen through a unit that `Conflicts=kodi.service`.
- All Pi-specific code lives behind traits in `src/platform/`.
- Output is 1080p60; use the femtovg renderer. Follow the Slint rules in IN-PROGRESS.md (no animated shadow size/blur).
- Never run store install commands without showing them first.
