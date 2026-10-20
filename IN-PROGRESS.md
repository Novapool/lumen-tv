# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-19

## Where we are
Lumen's UI is Slint (femtovg), 60 fps at 1080p60 and 4K60 on the TV. M3 done (2026-10-13): app registry with icon tiles; Jellyfin (Chromium kiosk), Steam Link and Kodi launch from Lumen; keep-alive/exclusive policies; Home works from any app and pauses its media; auto-return when an app dies. M3.5 done (2026-10-19): Settings/diagnostics screen (Display, Input, Apps, Running, Log, Toggles) and the LG TV remote drives everything on its own (red = Home). **Next: M4 Store.** Then M4.5 controllers + Bluetooth pairing screen → M5 boot session. Kodi still boots by default; Lumen runs from `lumen-test.service`.

## Active Plan
Build M4 → M4.5 → M5 in order (see MILESTONES.md), stopping after each one for a TV test.

Layout (as built; `store.rs`, `store.slint`/`details.slint` arrive in M4):
```
lumen-tv/
  Cargo.toml, build.rs     one crate; slint-build compiles ui/
  src/
    main.rs                window setup, Slint callbacks, Settings wiring, input gate, bench hook
    session.rs             glue: lifecycle + windows + tiles (UI thread)
    apps.rs                Lifecycle planner (pure, tested) + process control, MPRIS pause
    registry.rs, icons.rs  app TOMLs; icon lookup + bake onto tiles
    bake.rs, bench.rs      CPU-baked images; LUMEN_BENCH frame-time bench + FpsMeter
    diag.rs, logtail.rs    Settings panel data; capture of Lumen's own log for the Log panel
    reclaim.rs             re-claim the CEC remote after an app exits (debounced)
    input/                 evdev reader, hotplug, mapping loader, repeat/deadzone, dispatch
    platform/mod.rs        traits: Windows, Tv
    platform/pi.rs         CEC remote claim (cec-ctl) + OK/Back keymap remap
    platform/toplevel.rs   labwc wlr-foreign-toplevel
  ui/                      app.slint (screens, focus; Store tile does nothing yet), settings.slint, tile.slint, theme.slint
  config/input/            keyboard, xbox, playstation, gamepad, cec-remote (.toml)
  config/apps/             default registry entries; config/kodi-tile = Kodi tile launcher (CEC off)
  examples/                fake-pad.rs, press.rs, toplevels.rs (gate tools)
  session/                 labwc rc.xml, start-lumen.sh, lumen-test.service, sudoers
  scripts/pi-build.sh      rsync to pitv + cargo build --release there
```

Decisions so far:
- Registry in `~/.config/lumen-tv/apps/` (XDG config dir: user-editable, survives reinstalls, no root needed). Mapping defaults ship in the app; user overrides go in `~/.config/lumen-tv/input/`.
- Build Rust natively on the Pi (simplest; no cross toolchain).
- Window control through labwc's wlr-foreign-toplevel-management protocol (activate/close), behind `platform::Windows`.
- Input → UI: evdev reader thread calls `slint::invoke_from_event_loop` to deliver normalized events to UI callbacks.

## Settled
- (2026-10-08) UI is Slint 1.18, femtovg renderer, winit backend under labwc. Not Skia: slower at both resolutions, and its default wgpu/Vulkan surface never fires the rendering notifier.
- (2026-10-08) Lumen is GPLv3 (Slint's royalty-free license excludes embedded systems).
- (2026-10-08) Port before M2. Kiosk browser for `fallback_url` = Chromium (M3).
- (2026-10-08) 4K60 works with baked images (59.7 fps, 0.4% slow). Default output stays 1080p60 for now (Laith); `LUMEN_MODE=3840x2160@60` picks 4K.
- (2026-10-06) Kodi stays as a Lumen tile (close-on-exit, exclusive) for Games/YouTube/Mirror until replacements exist.
- (2026-10-06) Rust 1.99 via rustup (`~/.cargo/bin`) on the Pi and the Mac (not on the Mac's PATH). apt packages need Laith's sudo.
- (2026-10-06) Store manifest lives in this repo: github.com/Novapool/lumen-tv, `store/manifest.json` (raw URL once pushed).
- (2026-10-06) Pi 5 has 8 GB RAM. laith is already in the `input` group. TV native mode is 3840x2160; Lumen sets 1080p60 by default with `wlr-randr` (`LUMEN_MODE` overrides it).

- (2026-10-09) Clients run as children of Lumen in labwc (env XDG_RUNTIME_DIR=/run/user/1000, WAYLAND_DISPLAY=wayland-0, DISPLAY=:0 via Xwayland). All raise/close/fullscreen via foreign-toplevel:
  - Jellyfin: web client in Chromium (`http://192.168.5.174:8096`, app_id `lumen-jellyfin`), TV layout set once via `localStorage.layout = "tv"` in its profile (`~/.local/share/lumen-tv/browser/jellyfin`). Jellyfin Desktop (Flathub) was dropped as the launcher (still installed, for its icon): no GPU in its Mesa (software rendering, very slow) and its player showed subtitles with no video/audio.
  - Web apps get their own Chromium profile and app_id `lumen-<id>` (`{id}` in the browser command); otherwise a second launch hands off to the running Chromium and exits.
  - Plex: Chromium kiosk, app_id `lumen-plex`, ~1.7 GB (F1 is swallowed in rc.xml, so Chrome help no longer opens).
  - Steam Link: `steamlink` (Xwayland), app_id `SteamLink`; launcher script exits at once, real app `~/.local/share/SteamLink/bin/shell` reparented to PID 1.
  - Kodi: `kodi --windowing=wayland`, app_id `Kodi`, real app `kodi.bin`, close takes 4.5 s.

- (2026-10-09) Audio: PipeWire (`pipewire pipewire-pulse pipewire-alsa wireplumber`, user services) mixes for the HDMI output (vc4-hdmi-0, volume 100%). Without it Chromium's second audio stream got "Device or resource busy" and played silently (the Pi 5 HDMI PCM has no mixing). Kodi/Steam Link with pipewire-alsa still to be checked.
- (2026-10-09) Startup race: if Lumen starts before HDMI-A-1 is ready, `start-lumen.sh` can't set the mode and Lumen's window comes up tiny, top-left, not fullscreen (labwc "invalid output set for view"). Fixed in 2.1: `start-lumen.sh` waits up to ~5 s for the output, and Lumen asks for fullscreen on its own window if it comes up windowed.

- (2026-10-10) Registry (`~/.config/lumen-tv/apps/*.toml`, defaults in `config/apps/`, seeded on first run; fields in `src/registry.rs`, unknown keys rejected): `id`, `name`, `type` (`command`|`url`), `launch`, `url`, `fallback_url`, `background` (`keep-alive`|`close-on-exit`), `exclusive`, `icon`, `app_id`, `process` (real process name when the launcher exits at once, e.g. Steam Link's `shell`), `glyph`, `colors`, `order`. `url` apps run through `LUMEN_BROWSER` (default: Chromium kiosk with per-app profile and `--class=lumen-<id>`; `{id}` and `{url}` are substituted). Icon lookup: a path under `~/.config/lumen-tv/icons/`, else a hicolor theme name (svg, then largest png); on a miss the tile shows its glyph. No brand artwork ships: Plex keeps its glyph until Laith adds a file; Jellyfin's icon (`org.jellyfin.JellyfinDesktop`) resolves only while the Flathub Jellyfin Desktop stays installed.
- (2026-10-10) `session/labwc/rc.xml` swallows F1 and Super_L so clients (Chromium help) never see Home. The action must be `Execute command="true"`, since `None` clears the binding. Gate tools: `examples/press.rs`, `examples/toplevels.rs`.
- (2026-10-13) Home from a keep-alive app sends MPRIS Pause (`busctl --user`, players owned by the app's process tree). Reselect does not auto-resume (Laith).

- (2026-10-13) TV remote (LG basic, SIMPLINK on): Lumen claims `/dev/cec0` as plain Playback "Lumen" (`cec-ctl --playback --osd-name Lumen`, then active source 2.0.0.0); the LG vendor ID 0x00e091 forwarded nothing extra. Home/Settings/Exit and a held Back never reach the Pi (the TV opens its own menu), so **red = Home** (KEY_RED 398 > 247, never reaches XKB, no rc.xml swallow needed); keyboard F1 stays Home.
- (2026-10-13) Kernel keymap remap on every claim (EVIOCSKEYCODE_V2 on `vc4-hdmi-0`): OK 0x00 KEY_OK→KEY_ENTER, Back 0x0d KEY_EXIT→KEY_BACK, so Chromium/Jellyfin web act on them. `cec-remote.toml` lists post-remap codes plus KEY_OK/KEY_EXIT as a fallback.
- (2026-10-13) Kodi tile runs through `~/.local/bin/kodi-tile` (own home `~/.kodi-tile`, userdata linked to `~/.kodi`, CEC peripheral forced off) so Lumen keeps the remote; the boot Kodi is untouched. After any app exits Lumen re-claims the remote + active source (`src/reclaim.rs`). Backup `~/.kodi-backup-20261013`.
- (2026-10-13) Plex removed (Laith): Jellyfin covers it.

## Slint rules (from the spike)
- Never animate a drop shadow's size or blur: the blur re-renders every frame (56.5 fps, 5.2% slow). Keep the shadow box fixed and fade it with `opacity`. Slint draws no shadow for an unfilled Rectangle, so fill it and hide it behind the face.
- Slint redraws only on change: frame stats count only frames while something is animating.
- No "hold" animation workaround needed (that was a WebKit compositor bug).
- Radial gradients are circles only.
- Bake gradients, gloss and shadows into images (`src/bake.rs`); the Pi 5 GPU is fill-rate bound and can't draw them live at 4K.
- Slint drops a texture once nothing shows it, so keep the images that change per focus move small (heroes are 640×234 and scaled up).
- Never crossfade two full-screen layers on the GPU: blend the two baked backdrops on the CPU (`bake::blend`) and draw one Image.
- `if`-created drop-shadow elements re-blur every time they are created; use one fixed shadow Image and fade it.

## Current Work
- Job 1.0 done: Slint store contract (StoreItem/StoreJob, StoreScreen/DetailsScreen placeholders, AppWindow store props/callbacks, open/close only), `src/store.rs` stub, `mod store;` in main.rs. Mac cargo check/test pass (115 passed, 2 ignored); GUI not run.
- M4 Store planned (2026-10-19) in `AGENT_JOBS.md`. Phases: 0 decisions with Laith (D1-D7), 1A Slint contract stub, 1B UI ∥ store core ∥ live Home reload, 2 store wiring + first Pi build, 3 review/simplify/tests (+ optional O.1 mode fix), 4 automated Pi checks, 5 Gate at the TV. Phase 0 answers (Laith, 2026-10-19):
  - D1 (a): flatpak `--user` only; `root: true` entries show their commands with "run over SSH" and no Run button. A root helper with an on-screen PIN becomes its own milestone later (scope change in MILESTONES.md).
  - D2: KolourPaint (`org.kde.kolourpaint`, Flathub, runtime org.kde.Platform 6.10 = already installed, 1.3 MB). KCalc was the pick but now needs runtime 6.11 (~1 GB extra). Re-check the runtime with `flatpak remote-info --user flathub org.kde.kolourpaint` before Phase 4.
  - D3 (a): "Store test (fails)" (`org.lumen.DoesNotExist`) lives in the public `store/manifest.json`.
  - D4: push `store/manifest.json` to origin/main before Phase 4 (main conversation shows the commit first); `LUMEN_STORE_URL` overrides for local tests.
  - D5: no rollback (recommended default, not objected to).
  - D6: drop O.1. The 4K-after-power-cycle fix moves to M5 as a resolution switch in Settings (scope change).
  - D7: still pending: Laith OKs the exact manifest commands and the lumen-test stop/start before Phase 4.
- Job 1.1 done (2026-10-19, Mac check/test pass, GUI not run): Store list/progress in `StoreScreen` (levels 0, 3), Details/Confirm in `DetailsScreen` (levels 1, 2; shown from app.slint, since details.slint imports store.slint). Full `store-nav`; Confirm focuses Cancel. Open: a very long command list on Details isn't scrollable; text sizes unchecked at the TV.
- Job 1.3 done (2026-10-19, Mac check/test pass, 7 new planner tests; reload never run, Linux code never compiled): `src/home.rs` (`init`, `bake_tiles`, `reload -> Reloaded`, `with`), `Lifecycle::set_specs`/`close`, `session::set_entries`/`close_app`; Settings reads Home through `home::with`. For 2.1: after a reload the focused tile's hero backdrop (`hero-a`/`hero-b`) stays stale until the next focus move; re-assign it after `apps` is replaced.
- Job 1.2 done (2026-10-19, Mac 139 tests pass): `src/store.rs` + registry `write_entry`/`remove_entry`, `store/manifest.json` (jellyfin, steamlink root, kolourpaint, store-test). Contract deviations for 2.1: `run_job(action, app, on_event) -> Result<(), String>` reads `store::source()`; `JobEvent::Replace(String)` overwrites the last line (flatpak `\r`); `Manifest.fatal`; receipt/status fns take explicit dirs; one job at a time. To confirm on the Pi: KolourPaint app_id (`org.kde.kolourpaint` is a guess), whether `process` is needed, icon lookup from flatpak exports.
- Job 2.1 done (2026-10-19): `src/storefeed.rs` (`wire`), `--store-check [source]` in main.rs, hero fix (`changed apps` in app.slint). Mac 139 tests, Pi release build OK with no Linux-only compile errors, Pi 144 tests pass, `--store-check` on the Pi manifest: 4 apps, 0 errors, exit 0. KolourPaint runtime still org.kde.Platform 6.10. Unverified at runtime (lumen-test not restarted, GUI never run): the whole Store UI flow, reload live on the TV, hero refresh after reload, uninstall of a running app, `session::close_app`, real flatpak install, KolourPaint app_id/process/icon. Settings "Store source" row skipped (M5).
- Job 3.3 done: 5 tests added (non-string step, registry/receipt write failure, corrupt receipt, invalid id); Mac 144 pass; no bugs found.
- **M4 review (Job 3.1, 2026-10-19). Verdict: patch and ship, no High.** Every run path goes through Confirm; shown = run; failed install writes nothing; no UI state touched off-thread.
  - M1 installed `launch`/`url` never shown on Details/Confirm (a manifest could hide a command there). M2 uninstall starts before the app has exited (close_app only schedules SIGTERM; KolourPaint has no `process`); close also runs before `check`. M3 a step leaving a background child holding stdout hangs the job forever (no timeout, no cancel). M4 Confirm is bound live to `store-items[st-row]`: an earlier job finishing can flip Install to Uninstall under the cursor. M5 reopening the store mid-job resets to the list; a refused run still jumps to progress. M6 a partly failed uninstall never reloads Home; `remove_entry` Ok(false) still reports Done.
  - Low: L1 Details doesn't scroll long command lists; L2 stdout/stderr Replace clobber each other; L3 a tile can launch mid-uninstall; L4 "Step 1 of 0". FYI: uninstalling Jellyfin from the Store deletes the seeded TOML for good; reload re-bakes all tiles on the UI thread (hitch at 4K).
- **M4 simplify (Job 3.2):** two job-running guards (UI state + static BUSY) clear at different times → a fast re-run can be refused; drop `ItemStatus`/`status()`, stale `allow(dead_code)`, `Reloaded.errors`, `Feed.errors`; `read_receipt` should need only `app`; `last-row` property in store-nav; `first-line` in store.slint is always 0.
- Job 3.4 done (2026-10-19): M1-M6, L1, L2, L4 and the simplify list applied (L3 skipped); `StoreItem.launch` added; Details/Confirm scroll with Up/Down and bind to a snapshot (`st-item`); one job guard (storefeed `job.state == 1`; `BUSY` removed); `run_job(action, app, prepare, on_event)` waits up to 8 s for the closed app's PIDs; steps share one pipe, group-kill after 1 s grace, 15 min timeout. Mac 152 tests, Pi 158 pass, `--store-check` exit 0. GUI/runtime still unverified: Details scroll + wrapped-text layout height, uninstall wait on a real app (KolourPaint `process` not set), reopen-mid-job.

## Known issues
- After `systemctl stop lumen-test`, its ExecStopPost starts Kodi: wait until kodi is active before starting a test unit again.
- `ssh pitv` uses the IP (192.168.1.60), since `pitv.local` (mDNS) doesn't resolve over the VPN.
- `/dev/uinput` is writable by the input group: `cargo run --release --example fake-pad [secs]` on the Pi tests the input layer without a controller.
- Perf lines log under `labwc[...]`, so grep the whole journal, not `-u lumen-test`.
- Don't run wf-recorder on the Pi: its frames come out garbled and it froze the TV output.
- History: the WebKit focus-change flash/snap (compositor drew pre-animation values for ~2 frames) was patched with "hold" animations (95dc6f9); gone since the Slint port.
- Benchmark: put `LUMEN_BENCH=1` in `~/.config/lumen-tv/env` (runs on every start; remove it after), or press B on the home screen.

- Home (`KEY_F1`/`KEY_LEFTMETA`) is read from evdev, so the focused app also receives it unless rc.xml swallows it (F1 and Super_L are; `KEY_HOMEPAGE` is not).
- Steam Link needs Xwayland, and labwc must be restarted after installing it (restart `lumen-test`).
- Flatpak apps: the launched process is a wrapper, so descendants (`bwrap`/app) may outlive close; set `process` and check the terminate path before shipping one.

- Output mode resets to 4K after the TV power-cycles: labwc drops and re-adds HDMI-A-1 at its preferred 3840x2160, and only `start-lumen.sh` applies `LUMEN_MODE` (1080p60 default), once at start. Found at the M3.5 TV check (2026-10-19). Lumen holds 60 fps at 4K, so this is a perf/consistency issue, not a breakage. Fix candidate: re-apply the mode when the output reappears.

## Blockers
- (none)
- Bench is thermally sensitive: the Pi has no fan; after long builds it throttles at 75-82 C and 4K drops to 45-55 fps (a cool 4K run passed at 60). Let it cool before benching.

## Next Steps
1. **M4 Store kickoff (orchestrator).** Read MILESTONES.md M4 (scope + Gate), BRIEF.md's store section, this file's Settled / Slint rules / Known issues, and `../CLAUDE.md`. Write AGENT_JOBS.md. The Gate needs a manifest with at least one real installable app and one deliberately failing entry; every install/uninstall command is shown to Laith before it runs.
2. Decide whether to fold the 4K-after-power-cycle fix (Known issues) into M4 or leave it for M5.
3. Parked: controllers + Bluetooth pairing screen are M4.5; boot session is M5.

## Recently Completed
- M3.5 Settings/diagnostics + TV remote (2026-10-19): gate passed on the Pi and at the TV (all S/R/G checks; red = Home 5/5 from Jellyfin, Kodi, Steam Link; remote works after Kodi closes). Result line in MILESTONES.md, execution notes in docs/ARCHIVE.md.
- M3 Registry + launching + icons (2026-10-13): gate passed on the Pi and at the TV (Home 2–7 ms, same-PID reselect 5/5, exclusive kill 1.2 s, SSH kill → Lumen 23 ms, Steam Link smooth with sound). Late fixes: F1 swallowed in labwc (665c03d), Steam Link window swap (2458926), Home pauses media via MPRIS (bb61e09). Result line in MILESTONES.md.
- 4K bake merge (2026-10-08): `exp/baked-hero` merged into main (0abbe64). 4K60 59.7 fps / 0.4% > 25 ms / p95 17.4 ms (was 27.8 fps); 1080p60 60.0 / 0.1% / p95 17.1. M2 input kept; HDMI remote verified at 4K. Default stays 1080p. Branch, worktree and `~/lumen-bake` on the Pi removed.
