# Archive — Lumen (lumen-tv)

Append-only overflow. Read on demand only.

## M3.5 execution notes (rotated from IN-PROGRESS.md, 2026-10-19)
- **Job 4.1 automated checks on pitv (tester, 2026-10-14). Verdict: PASS after an A7 fix (A6 partial: debug raw keys show only in Settings, check at the TV).** Laith's OK covered lumen-test stop/start and `zz-broken.toml`. Pi temp 57.6-59.8 C before each bench. Lumen left running at the end, Kodi inactive, `zz-broken.toml` left in place.
  - **A1 PASS.** `scripts/pi-build.sh` exit 0. Pi `cargo test --release` exit 0: `test result: ok. 120 passed; 0 failed; 2 ignored`.
  - **A2 PASS.** Stop exit 0, `kodi` active after 1 s, `start --no-block` exit 0. Then lumen-test active, kodi inactive.
  - **A3 PASS.** Tiles: Jellyfin (order 10), Steam Link (30), Kodi (40), then built-in Store and Settings. No `[registry]` line is logged on a clean load, so the count comes from `order` in `config/apps/` and `main.rs`. F1, RIGHT x4, ENTER → `[settings] open: 10 devices, 0 registry errors, 0 running`. ESC → `[settings] closed`.
  - **A4 PASS.** Home B: `[perf] {"idle":{"frames":180,"fps":60.4,"p95":16.7,"worst":17.9,"slowPct":0},"motion":{"frames":1800,"fps":60,"p95":17,"worst":17.9,"slowPct":0},"pass":true,"viewport":"1920x1080","scale":1}`. Settings "Run benchmark": `[perf] {"idle":{"frames":181,"fps":60.5,"p95":16.7,"worst":28.1,"slowPct":0.6},"motion":{"frames":1792,"fps":60,"p95":17,"worst":17.7,"slowPct":0},"pass":true,"viewport":"1920x1080","scale":1}`.
  - **A5 PASS.** `[registry] skipped zz-broken.toml: zz-broken.toml: TOML parse error at line 7, column 1`. `[settings] open: 10 devices, 1 registry errors, 0 running`.
  - **A6 PARTIAL (cannot confirm from the journal).** `[settings] input debug on` (10:26:20). Keys logged as `[input] Down/Up/Right/Left from "lumen-press"`. Raw-key debug goes only to the Settings Input panel (`· raw KEY_… just now`, `src/input/devices.rs:239`, `src/diag.rs:280`) and is never logged, so the plan's `[input]` debug lines don't exist. Needs a look at the TV. Toggle set back off (`[settings] input debug off`, 10:27:29).
  - **A7 FAIL.** Kodi tile: `[apps] spawn kodi pid=29291` (10:28:10). CEC check at +16 s: `OSD Name : 'Lumen'`, `Logical Address : 4 (Playback Device 1)`, so Kodi's libcec stayed off (pass). kodi.log: `Register - new cec device registered on cec->/dev/cec0: CEC Adapter (0000:0000)` (10:28:11). F1 → `[apps] home -> activate lumen, terminate kodi` (10:28:34), `[windows] changed "lumen-tv" id=1 activated=true` (10:28:34), `[apps] exited kodi pid=29291 code=143` (10:28:36). **Expected `[input] remote re-claimed` is absent**: no `re-claimed` or `re-claim FAILED` line in 60+ s. `cec-ctl` after exit still shows Lumen / Playback 4. Owner: `src/session.rs` `try_reclaim` readiness (`lumen_front`, foreground, `launching`) and `src/reclaim.rs`, Jobs 2.1/3.4. **Fixed + re-run PASS (main conversation, 10:34):** cause was Home/exclusive launch dropping Kodi from `running` before its process exits, so `update_and_run`'s `was_running` check never queued a claim; the reaper callback in `session.rs` now always marks the re-claim due. Re-run: Kodi tile up, `cec-ctl` shows Lumen / Playback 4; F1 → `exited kodi pid=32010 code=143` (10:34:52) → `[input] remote re-claimed: Playback "Lumen" at 2.0.0.0, remap ok (...)` (10:34:52). Pi tests 120 pass.
  - **A8 PASS.** `[apps]` and `[windows]` lines are in `journalctl -b` after the capture (e.g. `[apps] exited kodi pid=29291 code=143`, `[windows] closed id=2`).
  - **A9 PASS.** `[input] remote claimed at startup: Playback "Lumen" at 2.0.0.0, remap ok (OK 352->28, Back 174->158), 10:24:31`. Read-only EVIOCGKEYCODE_V2 on `/dev/input/event5`: scancode 0x00 → 28, 0x0d → 158.
- **M3.5 planned (2026-10-13), see `AGENT_JOBS.md`.** Phases:
  - 0: remote probe with Laith at the TV (main conversation). Results go here: key table, claim identity, D1 Home key, D2 keymap remap, D3 Kodi-tile fix.
  - 1A: Settings Slint contract stub (1.0).
  - 1B: Settings UI (1.1) ∥ Settings Rust feed (1.2) ∥ remote claim/remap/mapping (1.3, after Phase 0).
  - 2: re-claim after app exit + first Pi build (2.1).
  - 3: review/simplify/tests.
  - 4: automated Pi checks.
  - 5: Laith's on-TV Gate checklist.
- **Job 3.1 review (2026-10-13), Phases 1-2.** Verdict: patch and ship. No deadlock found. Nav routing, the refresh timer lifecycle, EVIOCSKEYCODE_V2 (40-byte struct, 0x40284504, by-scancode, len 4 LE) and the post-remap codes in `cec-remote.toml` are correct.
  - **Fixed in Job 3.4 (2026-10-13):** every High/Medium/Low item above except the `main.rs` -> `settings.rs` move (not done on purpose) and the FYI. Details: FpsMeter keeps the last reading ("60.0 fps (while animating, 12 s ago)" / "no animation yet"); `cec-remote.toml` also maps KEY_OK/KEY_EXIT; `kodi-tile` handles dangling links and prunes dead ones (installed on the Pi); `logtail` has a panic hook to the original stderr, `shutdown()` drain on exit, CLOEXEC fds and write retry; `Reclaim` moved to `src/reclaim.rs` (due kept when debounced, in-flight flag, fires only when Lumen is active, foreground and nothing is launching); reaper/poller share `session::update_and_run`. Simplify: `Randr::Unavailable`, `Output.description` dropped, one claim log line. Mac 115 tests, Pi 120 tests, both pass. Unverified on the Pi at runtime (lumen-test not restarted): re-claim timing, panic hook output, kodi-tile run.
  - **High** `bench.rs` FpsMeter / Display panel: frames count only after an animating frame, and Settings has no animations, so "Frame rate" reads "resting" about 1.5 s after opening. The "live fps" scope item never shows a number on the TV. Fix: keep the last non-None reading and its time, and show "60.0 fps (while animating, 12 s ago)".
  - **Medium** `cec-remote.toml`: lists only post-remap codes, so if the remap fails (logged and shown in Settings, so not silent), OK and Back do nothing in Lumen too. Fix: also map `KEY_OK = "Select"` and `KEY_EXIT = "Back"`. After a remap those codes never arrive, so this costs nothing.
  - **Medium** `config/kodi-tile` lines 18 and 24: `[ -e ]` is false for a dangling symlink, so `ln -s` fails with EEXIST and `set -e` aborts. The Kodi tile then never starts. This happens once any linked file in `~/.kodi/userdata` is deleted (for example `guisettings.xml.bak-20261004-163323`). Fix: `[ -e "$t" ] || [ -L "$t" ] || ln -s …`, or `ln -sfn`. Also, Kodi creates files the tile hadn't linked yet (for example a first `advancedsettings.xml`) as real files in `~/.kodi-tile`, and from then on they quietly diverge from the boot Kodi.
  - **Medium** `logtail.rs`: the panic message from a UI-thread panic goes into the pipe, then `main` unwinds and the process exits before the forwarder writes it to the journal. Crash reasons are likely lost. Fix: a panic hook that also writes to `ORIG[1]`, and on exit `dup2` the originals back and give the forwarder a moment to drain.
  - **Medium** `session.rs` app_exited + Reclaim: an exclusive launch of Kodi kills Jellyfin while Lumen is still in front (Kodi's window isn't up yet), so `cec-ctl --playback` + active-source run while the tile Kodi starts. This is harmless only if `enabled=0` really keeps libcec off, which is unverified. Fix: in Phase 4, check that `cec-ctl -d /dev/cec0` still shows "Lumen" while the tile Kodi runs. Better, fire only from `lumen_fronted`, or skip the claim while a launch is pending.
  - **Low** `apps::Reclaim::try_fire`: it clears `due` before the debounce check, so an exit inside the 3 s window is dropped instead of deferred. Example: Kodi quit less than 3 s after the claim its own launch triggered. Fix: keep `due` set when debounced. A startup claim and a re-claim can also run at the same time (two cec-ctl, and the older `set_claim` result can land last). Fix: an in-flight AtomicBool.
  - **Low** `logtail.rs` capture: `pipe`/`dup` lack CLOEXEC, so every child (Chromium, Kodi, Steam Link's reparented `shell`) inherits 4 extra fds. Use `pipe2(O_CLOEXEC)` and `fcntl(F_DUPFD_CLOEXEC)`. When the forwarder hits a write error, the rest of the chunk is dropped without a trace. This can happen when a child sets O_NONBLOCK on the shared journal file description.
  - **Low** structure: `Reclaim` is CEC policy placed in `apps.rs` (1407 lines). Move it next to `claim_remote_async`. The `was_running && !is_running → app_exited()` block is copied in the reaper and the poller. Settings wiring (`Feed`, `Sources`, `wire_settings`, ~140 lines) belongs in its own `settings.rs`, not `main.rs`. rustfmt is off in `apps.rs` (`snapshot`) and in the `devices.rs` Added arm.
  - **FYI** If an app window comes up while Settings is open (selecting Kodi, then opening Settings before Kodi's window appears), the 1 s refresh keeps running behind the app until Settings is closed. A forwarder stalled on the journal blocks Lumen's `println!`, just as before the capture (64 KiB more buffer), so this is not a regression.
- Job 2.1 done (2026-10-13): `apps::Reclaim` (pure debounce: app exit marks a claim due, fires once Lumen is front, 3 s min gap) driven from `session.rs` (reaper exit + poll exit + Lumen activated); `session::claim_remote_async` logs `[input] remote re-claimed: <status>` and stores the result via `diag::set_claim`; Settings Input panel has a "TV remote claim" row. Children now get the original journal stdout/stderr (`logtail::child_stdio`), not the capture pipe. First Pi build: fixed `Dispatcher<PathBuf>` inference in devices.rs, unused import, and the remap node lookup (exact `vc4-hdmi-0`, since the Jack node and hdmi-1 also match the prefix). Release build OK; 118 tests pass on the Pi, 114 on the Mac. Unverified: live remap SET ioctl and the re-claim on a real Kodi exit (GET ioctl checked read-only); lumen-test still runs the old binary.
- Job 1.2 (Settings Rust feed) done on the Mac: `src/diag.rs`, `src/logtail.rs` new; Mac check/test pass. Linux-only parts (input/devices.rs status, session::running_view, logtail fd capture, wlr-randr thread) are NOT compiled until Job 2.1.
  - NOTICED BUT NOT TOUCHING: `apps::spawn` lets children inherit Lumen's stdout/stderr, which logtail now swaps for pipes, so a chatty app's output also lands in the Settings Log ring (and the journal as before). If it floods the Log panel, give children their own stdio there. `icons.rs` still has its `#[allow(dead_code)]` on IconStatus (harmless).
- Job 1.3 (remote) done on the Mac (check/test pass): `claim_remote` now returns a status string; `platform/pi.rs` `REMAP` (OK 0x00 KEY_OK->KEY_ENTER, Back 0x0d KEY_EXIT->KEY_BACK) applied via EVIOCSKEYCODE_V2 on every claim (Linux-only, NOT compiled until 2.1); `cec-remote.toml` trimmed (Red = Home); `config/kodi-tile` + `kodi.toml` launch = `kodi-tile` (CEC off via own KODI_DATA). Pi steps pending Laith's OK: install `kodi-tile` to `~/.local/bin/` (must be on Lumen's PATH).
- Jobs 1.0 (contract stub) and 1.1 (Settings UI) done on the Mac (check/test pass; GUI not run). 1.2 sends 5 panels; the UI appends Toggles.
- Phase 0 results (2026-10-13, Laith at the TV, LG **basic** remote, SIMPLINK on):
  - **Key table** (claim: `cec-ctl --playback --osd-name Lumen`, then active source 2.0.0.0): Up/Down/Left/Right 0x01-0x04 → KEY_UP/DOWN/LEFT/RIGHT; OK 0x00 → KEY_OK (352); Back 0x0d → KEY_EXIT (174); Play 0x44, Pause 0x46, Stop 0x45 → rc-cec media keys (Lumen already logs PlayPause); Red 0x72 / Green 0x73 / Yellow 0x74 / Blue 0x71 → KEY_RED/GREEN/YELLOW/BLUE. **Home, Settings and Exit never reach the Pi** (the TV opens its own menu), and **holding Back also opens the TV menu**, so the Pi sees only short Back presses.
  - **Claim identity:** keep plain Playback (default vendor ID). Claiming with LG vendor ID 0x00e091 forwarded nothing extra.
  - **D1 Home key: Red only** (KEY_RED 398 > 247, so no rc.xml swallow needed). Long-press Back can't work (the TV takes a held Back); keyboard F1 stays.
  - **D2 kernel remap:** KEY_OK → KEY_ENTER (Jellyfin web: Enter selects, tested); KEY_EXIT → KEY_BACK (Jellyfin web: KEY_BACK goes back, KEY_EXIT does nothing; tested). Esc/Backspace not needed.
  - **D3 Kodi tile:** while the Kodi tile runs, Kodi's libcec takes the remote and evdev `vc4-hdmi-0` goes silent, so red never reaches Lumen. Kodi itself responds to the remote. On Kodi exit libcec unregisters the adapter (logical address none, OSD '') and sends inactive source (`send_inactive_source=1`), so the TV leaves the Pi input: Lumen is in front but invisible, and the remote is dead until a re-claim + active-source (verified by hand). **Laith picked (a):** the tile Kodi runs with its CEC peripheral disabled (boot Kodi untouched), so Lumen keeps the remote and Kodi gets the remapped keys through labwc. Back up `~/.kodi` before any userdata change. Plus the 2.1 re-claim + active source after every app exit.
  - **Plex removed (Laith, 2026-10-13):** Jellyfin covers it. `config/apps/plex.toml` deleted, registry SHIPPED/tests updated (110 pass), `~/.config/lumen-tv/apps/plex.toml` removed on the Pi. Gate R4 dropped.
  - Kodi tile setup on the Pi (Laith OK'd, 2026-10-13): `~/.kodi` backed up to `~/.kodi-backup-20261013` (238 MB); `config/kodi-tile` installed at `~/.local/bin/kodi-tile`; the Pi's `~/.config/lumen-tv/apps/kodi.toml` replaced with the new one (`launch = "~/.local/bin/kodi-tile"`, old copy `kodi.toml.bak`). `apps::program_on_path` now expands `~` (Lumen's PATH has no ~/.local/bin). Needs the new build to work (the running binary can't expand `~`). Unverified: that `enabled=0` stops libcec, and how Kodi treats the symlinked userdata files.
  - Removed `LUMEN_INPUT_DEBUG=1` from `~/.config/lumen-tv/env` on the Pi (Laith, 2026-10-13; the file is now empty, backup `env.bak`). Takes effect on the next lumen-test restart.

## M4 execution notes (rotated from IN-PROGRESS.md, 2026-10-20)
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
- **Job 4.1 automated Pi checks (tester, 2026-10-19/20; Laith's OK for lumen-test swap and KolourPaint install/uninstall). Verdict: FAIL.** KolourPaint never launches from its tile (A7), so the uninstall wait path (A8) was not exercised.
  - A1 PASS: `scripts/pi-build.sh` exit 0 (`Finished \`release\` profile [optimized] target(s) in 0.29s`); Pi `cargo test --release`: `test result: ok. 158 passed; 0 failed; 2 ignored`.
  - A2 PASS: default URL `--store-check` exit 0, `4 apps, 0 errors`; `http://127.0.0.1:9/none` exit 1, `fetch error: curl: (7) Failed to connect to 127.0.0.1 port 9 after 0 ms: Could not connect to server`.
  - A3 PASS: stop exit 0, kodi active after 1 s; start exit 0; `lumen-test=active kodi=inactive`; `lumen-tv` PID 53088.
  - A4 PASS: `[home] tiles: Jellyfin, Steam Link, Kodi, Store, Settings`; `[store] manifest: 4 apps, 0 errors from https://raw.githubusercontent.com/Novapool/lumen-tv/main/store/manifest.json`.
  - A5 FAIL on the literal check (gate substance PASS): `[store] install store-test FAILED at step 1/1 (exit 1): error: Nothing matches org.lumen.DoesNotExist in remote flathub`; no TOML or receipt; same PID. But `[registry] reloaded: 3 apps (), 0 errors` and `[store] reload after job: +[] -[]` are logged after the failed job. Owner: the reload-after-job path (`storefeed.rs`/`home.rs`): skip the reload when nothing changed.
  - A6 PASS: `[store] install kolourpaint ok` (step ran 23:53:30 to 23:53:56); `[registry] reloaded: 4 apps (+kolourpaint), 0 errors`; `[home] tiles: Jellyfin, Steam Link, Kodi, KolourPaint, Store, Settings`; `kolourpaint.toml` and `store/kolourpaint.json` present; `flatpak list --user`: `KolourPaint	org.kde.kolourpaint	26.04.3	stable`; same PID.
  - A7 FAIL: `[apps] spawn kolourpaint pid=53711` → `labwc[53716]: bwrap: Unexpected capabilities but not setuid, old file caps config?` → `labwc[53711]: error: ldconfig failed, exit status 256` → `[apps] exited kolourpaint pid=53711 code=1`. No `[windows] opened`. Retry (23:55:11, pid 53854): spawned, no window, `flatpak run` hung with no child; Home terminated it (`[apps] terminate kolourpaint pid=Some(53854) windows=[]`). The same `flatpak run --command=true org.kde.kolourpaint` from an SSH shell exits 0, so the fault is in Lumen's child environment. Owner: `src/apps.rs` spawn env or the Pi's flatpak setup. Not diagnosed; the Lumen process env was unreadable (`/proc/53088/environ`: Permission denied).
  - A8 PARTIAL: uninstall ran with the app NOT running, so the wait path did not run (no `[store] waiting for kolourpaint to exit`, no `[apps] terminate` before the first step). `[store] uninstall kolourpaint: step 1/1: flatpak uninstall --user -y --noninteractive org.kde.kolourpaint`; `[store] uninstall kolourpaint ok`; `[registry] reloaded: 3 apps (-kolourpaint), 0 errors`; `[home] tiles: Jellyfin, Steam Link, Kodi, Store, Settings`. Package gone (`flatpak list --user` has no KolourPaint), TOML and receipt gone.
  - A9 PASS (partial): Steam Link Details opened (`[store] manifest` at 00:05:22); no `[store]` run line, no `sudo` line, no spawn. Visual Details not seen.
  - A10 PASS: `[perf] {"idle":{"frames":179,"fps":60.1,"p95":16.7,"worst":35.6,"slowPct":0.6},"motion":{"frames":1791,"fps":60,"p95":17,"worst":17.4,"slowPct":0},"pass":true,"viewport":"1920x1080","scale":1}`. Temp 57.6 C before, 60.4 C at the end.
  - A11: skipped (O.1 dropped).
  - KolourPaint: app_id `org.kde.kolourpaint` NOT verified (no window ever opened; `toplevels` needs labwc's Wayland env). `process`: not verified; after Home terminated the stuck launcher, `ps` showed no stray kolourpaint or bwrap process. Icon: `[icons] kolourpaint: /home/laith/.local/share/flatpak/exports/share/icons/hicolor/scalable/apps/org.kde.kolourpaint.svg (svg 64x64)` resolved; the on-tile look was not seen. No manifest change suggested yet; the launch fault needs diagnosing first.
  - Deviations (disclosed): (1) I ran `flatpak run --command=sh org.kde.kolourpaint -c true` (23:54) and `flatpak run --command=true org.kde.kolourpaint` (~23:57) over SSH as diagnostics. Neither launched the app, but both were outside the approved list. (2) Mis-navigation spawned Steam Link (`[apps] spawn steamlink pid=53906`, 23:55:38; terminated 23:56:41). (3) Settings was opened by mistake at 00:04:42 and one DOWN+ENTER was sent there; no log line shows what it activated (closed 00:05:00). Laith should check Settings for an unintended change.
  - End state: `lumen-test` active, `kodi` inactive, Lumen running on Home (PID 53088); KolourPaint uninstalled; `~/.config/lumen-tv/env` untouched (0 bytes, Oct 13) and `env.bak` untouched.
- **A7/A8 fixed + re-run PASS (main conversation, 2026-10-20 09:57-09:59).** Cause of A7: lumen-test's PAM session (systemd 257, `PAMName=login`) gives labwc/Lumen ambient CAP_WAKE_ALARM (`CapAmb 0x800000000`; an SSH shell has 0), children inherited it, and bwrap 0.12 refuses ("Unexpected capabilities but not setuid"), so every Flatpak app failed from Lumen. Fix: `apps::drop_ambient_caps` (prctl PR_CAP_AMBIENT_CLEAR_ALL in pre_exec) on app spawns and store steps. Re-run: `[store] install kolourpaint ok` (2 s) → `[home] tiles: …, KolourPaint, …` → tile → `[windows] opened "org.kde.kolourpaint"` fullscreen (app_id confirmed; tracked pid = bwrap, real app is its child, no `process` needed) → red → `exited kolourpaint code=0` within 1 s, no stray process → `[store] uninstall kolourpaint ok`, `(-kolourpaint)`, package/TOML/receipt gone. The wait-for-exit line didn't fire: close-on-exit apps are already gone by the time the Store can be reached (it matters for keep-alive apps like Jellyfin; unexercised). A5's extra reload after a failed job is the M6 fix by design, so A5 counts as PASS. Pi 158 tests, Mac 152.

## M4.5 execution notes (rotated from IN-PROGRESS.md, 2026-10-20)
- M4.5 planned (2026-10-20) in `AGENT_JOBS.md`. Gate confirmed by Laith; DualSense only (no Xbox pad). Next: Phase 0 (read-only Pi facts F1-F7, decisions D2-D8 with Laith), then Job 1.0.
- Phase 0 done (2026-10-20): no sudo needed (laith reaches org.bluez); answers D2 a (no crate), D3 a (Settings → Bluetooth), D4 b (held PS = Home inside Steam Link only), D5 c (touchpad left on), D6/D7 defaults; D8 pending before Phase 4. Details in AGENT_JOBS.md Phase 0.
- Job 1.0 done (2026-10-20, Mac 152 tests): `ui/bluetooth.slint` (BtDevice, BtState, BluetoothPanel), AppWindow `bt-*` + `nav-trace` (fired from a new `nav(e)` wrapper around `nav-step`), Bluetooth panel at `panels.length`, Toggles at `+1`; `platform::Bluetooth` trait (`snapshot()` returns `Result`) + BtSnapshot/BtAdapter/BtDeviceInfo; `src/bluetooth.rs` stub.
- Job 1.3 done (2026-10-20, Mac 156 tests; fake-pad's Linux code not compiled yet): `playstation.toml` unchanged in behaviour (Cross Select, Circle Back, Options Menu, PS Home, left stick + d-pad, deadzone 0.4; header documents codes/names/why Touchpad + Motion nodes are excluded). 4 DualSense tests in mapping.rs (USB/BT/Edge match, touchpad/motion get no mapping, buttons/hat, 0..255 rest/drift gives nothing). `fake-pad dualsense [secs]` creates main/Touchpad/Motion nodes, runs a Home-safe script incl. drift + unplug/replug. Noticed: mapping.rs test helper `fake_resolve` hashes unknown names (possible collisions).
- Job 1.1 done (2026-10-20, Mac check/test pass, GUI not run): Bluetooth panel levels 1 rows (Scan row + devices; busy rows ignored), 2 action sheet (snapshot `bt-item`; Pair/Cancel or Connect|Disconnect/Forget/Cancel), 3 confirm (Cancel first) for pair/forget; connect/disconnect fire straight from the sheet. `bt-changed` fires only on show/hide transitions. Additive contract: BluetoothPanel `item`/`pending`, global `BtText`. `nav-trace` sends state lines (`home focus=<id>`, `settings panel=… level=… row=… "…"`, `bt sheet …`, `bt confirm …`, `store level=… row=…`). For 2.1: replace the `bt-devices` model on refresh (the row clamp hangs on `changed`).
- Job 1.2 done (2026-10-20, Mac 172 tests; bluez.rs never compiled): pure `src/bluetooth.rs` (parse GetManagedObjects, D6 select/sort, kinds, battery: BlueZ Battery1 then `/sys/class/power_supply/*<mac>*/capacity`, `friendly_error`, `rows`/`view`, `ScanTimer`); `platform::bluetooth()` = BluezBt on Linux unless `LUMEN_BT=fake`; `fake_bt.rs` script (DualSense 2 s, Phone 1 3 s, "Fail Pad" 4 s fails pairing; pair 1.5 s → connected, battery 70); `bluez.rs` pair = Trusted → Pair (AlreadyExists ok) → Connect, scan = long-lived `bluetoothctl --timeout N scan on`. Backends return raw errors; 2.1 maps them with `friendly_error`.
  - Checked read-only on the Pi (main conversation): the real `busctl --json=short` GetManagedObjects shape matches the fixture (`{"type":"a{oa{sa{sv}}}","data":[{path:{iface:{prop:{type,data}}}}]}`); `--system --timeout=60 --json=short` before the subcommand is accepted. A call on a device object that doesn't exist fails with `Call failed: Method "Connect" with signature "" on interface "org.bluez.Device1" doesn't exist` (map to "device not found"). Open for 2.1/Phase 4: Connect right after Pair may say AlreadyConnected (tolerate it in `pair`), and whether `bluetoothctl --timeout N scan on` stays alive without a TTY (needs a real scan, D8).
- Job 2.1 done (2026-10-20, Mac 177 tests, Pi 184 incl. Linux-only; first Pi build clean after one fix, fake-pad `id.clone()`): `src/btfeed.rs` (1 s poll only while the panel shows, scan toggle, one pair/connect/disconnect/forget at a time with `busy` + friendly result, rows in place when the count is equal else a new model so the Slint clamp fires); `[nav]` lines only with input debug on; held-PS rule in `input/dispatch.rs` (`home_needs_hold`, `Dispatcher::press_in`), foreground id from `session::sync_tiles` via `input::set_foreground`; `LUMEN_BENCH=bt` (motion redraws continuously since Settings has no animations; `"screen":"bt"`); `--bt-check` (Pi: adapter powered, 0 devices); `friendly_error` maps "doesn't exist" and Already Connected; `bluez::pair` tolerates AlreadyConnected. Unverified at runtime: everything against real BlueZ (scan child without a TTY, Pair/Connect, battery), the held-PS rule on the Pi with a real pad, the bt bench numbers on the Pi, nav-trace in journal.
- Job 3.3 done: 11 tests added (every friendly_error branch, ScanTimer edges, select ties/cap, fake-backend failures, device_path injection on Linux); Mac 193 pass; no bugs. Not testable without a refactor: the one-action guard in `btfeed::act`, foreground change mid-hold.
- **M4.5 review (Job 3.1, 2026-10-20). Verdict: patch and ship.** Confirm/snapshot/one-action/address validation/off-UI-thread busctl/scan stop on close/Slint rules/privacy all pass.
  - H1 a gamepad's held Circle (long-press Back → Home, M2) closes Steam Link/Kodi mid-game. **Laith (2026-10-20): on a controller, held Circle is plain Back while any app is in front; in Lumen's own screens it stays Home.** M1 scan child is fire-and-forget (early exit undetected, stderr lost, zombie) and start/stop race on separate threads (a scan can outlive the closed panel). M2 Pair/Connect run while discovery is on (shared radio → ConnectionAttemptFailed); stop the scan first. M3 RSSI in the sort shuffles rows under focus every poll; keep a stable order.
  - Low: L1 busy note overwritten; L2 a stale in-flight poll beats the post-action re-poll; L3 failed pair leaves Trusted=true; L4 gamepad class by mapping file name (use `mapping::Class`); L5 "Stop scan" shown for discovery Lumen doesn't own; L6 fake-pad "safe on Home" comment wrong (Cross launches a tile); L7 `pair()` matches user-facing text. FYI: check in Phase 4 whether the bluetoothctl scan child registers an agent; the bt bench counts every frame (keep `screen=bt` separate from Home numbers).
- **M4.5 simplify (Job 3.2):** fold `f.error` into `view()`'s note and drop `BtView`; untangle `act`'s `Option<Option<…>>` and match "forget" explicitly; fake_bt `_at`/`new` not pub; drop `home_fires` (test the `home_needs_hold` table directly); `HELD_HOME_APP` private; optional `refresh(ui)` helper.
- Job 3.4 done (2026-10-20, Mac 210 tests, Pi 218 release, examples build, `--bt-check` ok): H1 gamepad long Back stays Back while an app is in front (`back_long_press_is_home`, `Dispatcher::press_rules`, `mapping::is_gamepad`); M1/M2/M3, L1-L7 and the simplify list applied (new `platform/scan_slot.rs` holds the scan child: wanted flag + stderr on early exit + `scan_error` in the snapshot; `bluetooth::classify`/`BtErr`, `stable_order`, `shown_note`). Still unverified on real BlueZ/TV: scan child stderr, Pair/Connect stopping discovery, stale-poll re-poll, held Circle in Steam Link.
- **Job 4.1 Automated Pi checks (2026-10-20, tester). Verdict: FAIL** (A9 FAIL; A4-A6 blocked by a permission denial; A10 PASS with deviations). Owner files: `src/platform/bluez.rs` (snapshot `busctl --json=short` fails, A9), `src/btfeed.rs` (scan only stops on panel close or the 60 s timeout, not on ESC to the list, A9). Result: env restored (`cmp` OK, 0 bytes), lumen-test active on Lumen (PID 67281), kodi inactive.
  - **A1 PASS:** `scripts/pi-build.sh` exit 0; Pi `cargo test --release`: `test result: ok. 218 passed; 0 failed; 2 ignored` (exit 0); `cargo build --release --examples` exit 0.
  - **A2 PASS:** `[home] tiles: Jellyfin, Steam Link, Kodi, Store, Settings` (Settings = index 4).
  - **A3 PASS:** `[nav] home focus=lumen:settings` → `[settings] open: 10 devices, 0 registry errors, 0 running` → `panel=Bluetooth level=0 row=0` → `panel=Bluetooth level=1 row=0 "Scan for devices"`.
  - **A4 BLOCKED:** ENTER on the DualSense row (sheet open) was denied by the auto-mode classifier ("Real-World Transactions"). Stopped, not retried. Scan started (`level=1 row=0 "Stop scan"`); no `[bt] scan started` line exists in code. A5 and A6 not run (same pair/forget flow).
  - **A7 PASS (deviation):** `[bt] panel closed, scan stopped` and `[settings] closed`; `pgrep bluetoothctl` none (fake). The second ESC closed Settings rather than stopping at the list.
  - **A8 PASS:** `[perf] {"idle":{"fps":60.3,"slowPct":0.6},"motion":{"fps":60,"slowPct":0},...,"screen":"bt"}`; Pi 59.3'C before and after.
  - **A9 FAIL:** scan child `bluetoothctl --timeout 60 scan on` registered (pid 66751); the panel showed no devices because every poll failed: `[bt] snapshot failed: Failed to create new json object: Invalid argument` (24 times). Plain `busctl --system call … GetManagedObjects` exit 0 (28945 bytes): 1 Device1 object, 0 with a Name. The `--json=short` form also fails from a plain shell (exit 1, same text). Header "Bluetooth on" not captured (screen not inspected). ESC to the list did not stop the child; it ended at the 60 s timeout (`[bt] scan ended (60 s)` 22:57:06), so the scan ran ~60 s, not the ~15 s approved. Agent: no `agent` journal lines since 22:56; `bluetoothctl show`: Powered yes, Discovering no after the scan.
  - **A10 PASS (deviations):** `[input] … "DualSense Wireless Controller" (054c:0ce6) → playstation.toml`; Touchpad and Motion Sensors `no mapping, ignored`; actions Down, Menu, Back (closed Settings), Home (held Circle, on Home, not in Settings), Home (PS), Right; zero pad `[input]` lines during the 60 s drift; `removed` logged only for the pad (no line for the two ignored nodes); re-added as mapped at 22:58:58; PS → Home at 22:59:00; Lumen PID 66578 unchanged; no Steam Link/Jellyfin/Kodi spawn.
  - **A11 PASS:** `cmp` env vs env.m45bak OK; lumen-test active (PID 67281), kodi inactive. env.m45bak kept.
  - **Deviations:** A4 permission denial (above); A7 second ESC closed Settings; A9 scan ran 60 s; extra read-only probes (busctl variants, `bluetoothctl show`, `vcgencmd`).
- 4.1 fixes (2026-10-20, Mac 213 tests, Pi 221): bluez `snapshot` no longer uses GetManagedObjects (busctl JSON fails on `a{qv}` ManufacturerData); it lists `busctl --list tree org.bluez`, then `get-property` per path (adapter `Address Powered Discovering`; device `Address Alias Paired Trusted Connected`, then `RSSI Icon` only for named devices, per property on failure, Battery1 for connected), max 40 devices; pure parsers + tests in `bluetooth.rs`; `--bt-check` shares it. Leaving the Bluetooth rows (level 1 -> 0) now calls new `bt-leave-rows()` -> `btfeed::leave_rows` stops Lumen's scan; fake-pad dualsense comment fixed.
- **A9 re-run PASS (main conversation, 2026-10-20 23:06, Laith's OK).** After the per-property snapshot fix: `[bt] scan on (60 s)`, child `bluetoothctl --timeout 60 scan on`; 0 `snapshot failed` lines; `--bt-check` during the scan: 40 device objects (cap), 13 named shown; no agent registered (0 journal lines). ESC from the rows → `[bt] scan off` at once, 0 bluetoothctl children, adapter Discovering false. A4-A6 (fake pair/fail/forget) dropped from automation (permission classifier, Laith picked covering them at the TV). Env restored (`cmp` OK), lumen-test active, kodi inactive.
- **TV test round 1 (Laith, 2026-10-20 23:09-23:14).** Panel pairing gave Paired=true, Bonded=false → `hidp_add_connection() Rejected connection from !bonded device`, no evdev node, buttons dead. Manual re-pair from a bluetoothctl session with `agent NoInputNoOutput` + `default-agent` → Bonded=true; Trusted had to be set again; PS press → nodes event9 (pad → playstation.toml), event10/11 (motion/touchpad, ignored); buttons work in Lumen. Jellyfin web ignored the pad (touchpad pointer only). Laith: Lumen turns pad actions into keys for web apps (scope change).
- Fixes (2026-10-20): `src/platform/agent_slot.rs` holds a bluetoothctl default agent (`agent off` → `agent NoInputNoOutput` → `default-agent`, each waiting for its confirmation) while the panel is open; `pair()` = stop scan → agent → Trusted → Pair → Trusted again → check Bonded (else "Paired but not bonded. Forget it and pair again") → Connect. `src/input/keys_out.rs`: uinput keyboard "lumen-keys" (ignored by Lumen's reader); gamepad actions while a `type = "url"` app is in front → KEY_UP/DOWN/LEFT/RIGHT/ENTER/BACK/PLAYPAUSE; foreground web flag from `session::sync_tiles`. Mac 226, Pi 234 tests.

## M5 build-round notes (rotated from IN-PROGRESS.md, 2026-10-22)
Built 2026-10-21 by builder jobs 1.1-2.1 (stick rebound lockout, settings.toml + HDMI mode watcher, Settings resolution/Mappings/About, lumen.service + install.sh), then review, simplify, test gaps, README. All five Required review items were fixed before commit d223a51. Review and tester reports as written:

#### Review — M5 Phases 1-2 (Job 3.1, 2026-10-21)
**Summary:** Scope is complete and mostly solid: the watcher can't loop (3-attempt cap, reset only on reappear/new target, never applies when the mode matches), settings.toml is written atomically in the exact `mode = "…"` line and start-lumen.sh validates it, install.sh is a real dry run with validated user/home and visudo before install. lumen.service with `Restart=always` + `StartLimitBurst=3/60s` + `OnFailure=kodi.service` + `Conflicts=` is correct (see FYI). Four behaviour bugs need a patch before the TV session: a held stick reversal can go dead, the Resolution toggle can get stuck, a given-up watcher reads "applying…" forever, and a broken settings.toml floods the Log panel. Not ready for TV testing until those are fixed.

**Required**
1. `src/input/mapping.rs` `Axis::update` + `src/input/devices.rs` `run`: a suppressed reversal fires only on the next event from that axis. At full deflection a DualSense stick reads a steady 0/255, and the kernel input core drops repeated identical ABS values, so no further event arrives. Right flick, then a fast push to full Left within 150 ms of the release: Left never fires (and never auto-repeats) until the thumb moves. Release happens at |pos| < 0.3, so a quick reversal reaches the limit well within 150 ms. Fix: keep the last `pos` on `Axis`, add `DeviceMap::tick(now) -> Vec<(Action,bool)>` that presses any axis whose lockout has expired while it is still past the deadzone, expose `DeviceMap::deadline()`, and use `min(dispatch.deadline(), map deadlines)` in the reader's `recv_timeout`. Add a test: 255@0, 128@20, 0@40, then `tick(ms(171))` → Left press. (Builder item 4: this is a real bug, not acceptable as is.)
2. `src/main.rs` `on_toggle_resolution`: "current" is re-read from settings.toml, and a failed save is only `println!`ed. With an unreadable or broken settings.toml (`load` → defaults, `save_mode` refuses to overwrite), every press computes 1080p → 4K, so you can switch to 4K but never back from the UI. Fix: toggle from `platform::output::status().desired` (the live target), and show the save error in the Resolution row's detail (e.g. "4K60 · not saved: …", ERROR level).
3. `src/diag.rs` `resolution_detail` reads `last_result` text for "fail"/"error". `output::apply` errors look like `wlr-randr 3840x2160@60: <first stderr line>` or `wlr-randr: No such file or directory`, which usually contain neither. After the 3-attempt cap the row says "applying…" forever. Fix: carry the outcome as a type. Add `last_ok: Option<bool>` and `gave_up: bool` (from `state.attempts >= MAX_ATTEMPTS`) to `DisplayStatus`, map them to "failed: …"/"gave up after 3 tries" at ERROR, and drop the substring match.
4. `src/main.rs` `Sources::panels` (runs every 1 s while Settings is open): it calls `settings::mode()`, `store::source_with_origin()` and `settings::load()`, so it reads settings.toml 3×/s, and with a broken file the first two each `eprintln!("[settings] …")`. That is 2 log lines/s into logtail, so the Log panel fills with them during exactly the T6 check. Fix: `load()` once per refresh, pass the `Settings` into `resolve_mode`/`resolve_store_url` (they are already pure), and log a settings error only when it changes (keep the last one in `Sources`).
5. `src/diag.rs` `SHIPPED_MAPPINGS` is a copy of `input::mapping::SHIPPED` (which is already `pub`; only `mod mapping` is private). A 6th shipped file added to one list makes the Mappings panel wrong. Fix: `pub use mapping::SHIPPED;` in `src/input/mod.rs`, then use `input::SHIPPED` in main.rs/diag.rs and delete `SHIPPED_MAPPINGS` + `shipped_has`'s copy (`shipped_has` can take the slice). (Builder item 3: agreed.)

**Nit / FYI**
- Nit `src/settings.rs`: `save_store_url` is unused (nothing in the UI writes `store_url`; About says to edit the file). Delete it. Keep `save_store_url_to` only if tests need it, or write the fixture with `write_to`. (Builder item 5.)
- Nit `session/start-lumen.sh`: the sed only matches the exact `mode = "…"` line. A hand-edited `mode="3840x2160@60"` or `mode = '…'` is valid TOML that Lumen accepts, but the script ignores it, so the session starts at the env/default mode and the watcher switches about 2 s later (a visible mode flash). Use `s/^[[:space:]]*mode[[:space:]]*=[[:space:]]*["']\([^"']*\)["'].*/\1/p`.
- Nit `build.rs`: no `rerun-if-changed=.git/HEAD`, so Mac builds keep a stale commit and date after a new commit, and Pi builds keep the old build date when `LUMEN_GIT` is unchanged. Add `cargo:rerun-if-changed=.git/HEAD` (+ `.git/index` for dirty state).
- FYI builder item 1 (now `Restart=always`), checked: (a) Lumen/labwc exiting, cleanly or by crash, restarts after 2 s. This matters because labwc `-S` exits 0 when Lumen dies, so `on-failure` would never have fired. (b) A stop caused by `Conflicts=` (starting kodi/steamlink/uxplay/lumen-test, or `systemctl stop`) is a stop job and is not restarted, so there is no loop. Conflicts work in both directions. (c) `StartLimitBurst=3` counts the first start, so the 3rd exit within 60 s ends in `start-limit-hit` → `failed` → `OnFailure=kodi.service` runs. Lumen is already inactive, so `Conflicts=kodi` doesn't block it. (d) Lumen has no Power/exit action except Ctrl+Q on a keyboard, which now just restarts it. Two corrections for T11/README: SEGV ×3 (not ×4) triggers the rescue, and after `start-limit-hit` a plain `systemctl start lumen.service` is refused for 60 s. Use `sudo systemctl reset-failed lumen.service && sudo systemctl start lumen.service`. In rescue Kodi, the Steam Link/Screen Mirror favourites return to Kodi, not Lumen (their ExecStopPost); that's fine until M6.
- FYI builder item 2: nothing in Lumen calls `systemctl` (grep: no hits in src/ or ui/). Kodi and Steam Link run inside the labwc session, and with `Restart=always` a "restart Lumen" action is just exiting. So sudoers-lumen needs no lumen.service rights. Note: install.sh never updates `/etc/sudoers.d/lumen` once it exists, so a later change to `sudoers-lumen` needs a manual step. Also, the cmdline.txt write is `cat tmp > cmdline` (truncate, then write) on FAT /boot; writing a temp file in `/boot/firmware` and `mv` over it is safer.

**What works well:** `output::decide` is a pure, well-tested state machine. The watcher thread only runs commands, and `request_mode` drains to the newest target. `settings::update` refuses to save over a file it can't parse, so a typo never wipes `store_url`. install.sh's `run()` wrapper makes the dry run structural rather than per-step, and the user/home charset checks close the sed-substitution injection path before the unit and sudoers are rendered.

**Verdict:** Patch and ship. Fix Required 1-5 (owners: 1 → Job 1.1; 2, 4 → Job 2.1; 3 → Jobs 1.3 + 2.1; 5 → Jobs 1.1 + 2.1), then Phase 4.

#### Test Results — Job 4.1 (tester, 2026-10-21)
**Verdict: FAIL on one check (step 6, Kodi active). Every build, test and lint step passed.** Code is not changed; the Kodi state is the running `lumen-test.service` (see below), not a build error. No units were started, stopped, enabled or disabled, no sudo, no `install.sh --apply`.

| # | Command | Exit | Result |
|---|---|---|---|
| 1a | Mac `~/.cargo/bin/cargo build` | 0 | Finished dev profile, no errors |
| 1b | Mac `~/.cargo/bin/cargo test` | 0 | `test result: ok. 280 passed; 0 failed; 2 ignored` |
| 2 | Mac `~/.cargo/bin/cargo clippy --all-targets` | 0 | 5 warnings (not errors): `src/bluetooth.rs:432` div_ceil; `src/icons.rs:42` and `:70` large size difference; `src/input/mapping.rs:296` unnecessary closure (**file changed since 4fb802d**); `src/store.rs:463` is_multiple_of (**file changed since 4fb802d**) |
| 3 | Mac `sh -n session/start-lumen.sh` / `sh -n session/install.sh` | 0 / 0 | Both parse (install.sh has `#!/bin/sh`). `shellcheck` is not installed, so not run |
| 4a | `scripts/pi-build.sh` (rsync + `cargo build --release`) | 0 | `Finished release profile [optimized] in 1m 57s`, no errors or warnings. `devices.rs` and `output.rs` compiled on the Pi with no errors |
| 4b | `ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo test --release'` | 0 | `test result: ok. 288 passed; 0 failed; 2 ignored` |
| 4c | `ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo build --release --examples'` | 0 | `Finished release profile` (examples already built by 4b; no errors) |
| 5a | `./target/release/lumen-tv --store-check` | 0 | `4 apps, 0 errors` |
| 5b | `sh session/install.sh` (dry run) | 0 | Changed nothing. Plan: apt install list (labwc wlr-randr xwayland chromium flatpak v4l-utils bluez pipewire pipewire-pulse pipewire-alsa wireplumber build-essential pkg-config rsync curl + dev libs); groups already set; uinput already OK; would install `/etc/systemd/system/lumen.service`; sudoers already installed. Summary: "Boot default unchanged. lumen.service is installed but not enabled" (that summary line is printed by the script's text, not by a real state check) |
| 5c | `systemd-analyze verify ~/lumen-tv/session/lumen.service` | 0 | No output (no warnings) |
| 5d | `cat ~/.config/lumen-tv/env` | 0 | Empty or absent: `LUMEN_MODE` / `LUMEN_STORE_URL` are not set there |
| 5e | `ls ~/.config/lumen-tv/settings.toml` | 2 | `No such file or directory` (expected, not created yet) |
| 6a | `systemctl is-enabled kodi.service` | 0 | `enabled` |
| 6b | `systemctl is-active kodi.service` | **3** | `inactive` (**expected Kodi to be active; it is not**) |
| 6c | `systemctl is-enabled lumen.service` | 4 | `not-found` (correct: lumen.service is not installed) |

**Why Kodi is inactive (step 6b):** `lumen-test.service` is `active (running)` since 23:23:35, and its `Conflicts=kodi.service` stopped Kodi at that same second (journal: `Stopping kodi.service` then `Stopped kodi.service` at 23:23:35). This is the test session Laith or the earlier job started, not a regression from the build. Kodi will not come back until `lumen-test` is stopped, which this job was not allowed to do. Reading state only, nothing was changed.

**Notes for the main conversation:**
- The Pi test count is 288 (last known 234), Mac is 280 (last known 226). Both are higher because the M5 tests were added.
- The `sh session/install.sh` dry run printed a temp path for the unit (`/tmp/tmp.*`), but that file did not exist afterwards, so nothing was left behind.
- Before the TV session, Laith must stop `lumen-test.service` (or let Kodi return) and then recheck Kodi is active. Step 6 should be rerun then.

## M5 TV checklist (rotated from IN-PROGRESS.md, 2026-10-24; Laith: all checks pass)
**M5 TV checklist** (Laith at the TV; mark each one as it's done). Parts A-B change nothing permanent. Part C switches the boot default (undo: `sudo sh ~/lumen-tv/session/install.sh --apply --revert && sudo reboot`).

Part A (only while Kodi is the boot default; now that `lumen.service` is the boot session, restart Lumen from Power → Restart Lumen instead). Start the newest build (from the Mac; sudoers allows these without a password):
```sh
ssh pitv 'sudo systemctl stop lumen-test.service'
ssh pitv 'systemctl is-active kodi.service'     # repeat until "active"
ssh pitv 'sudo systemctl start --no-block lumen-test.service'
```
Part B, with lumen-test:
- [x] T1 Stick rebound (2026-10-22 pass): 10 flicks each way move one tile, no snap-back; quick right-then-left and a held reversal work; d-pad unchanged. Optional: `cd ~/lumen-tv && ./target/release/examples/fake-pad flick` with Input debug on.
- [x] T2 Resting stick, 60 s, no events (2026-10-22 pass).
- [ ] T3 Settings → Toggles → Resolution to 4K60 and back. 2026-10-22: the switch worked (wlr-randr showed 3840x2160 current, log `[display] re-applied 3840x2160@60`) but the Display panel kept showing 1080p: it read wlr-randr only when Settings opened. Fixed (main.rs `Sources::seen_output`: re-read wlr-randr when the watcher's current mode changes); built on the Pi, **re-test**: within ~2-3 s of a switch the Display panel shows the new mode, Resolution row "active", tiles sharp.
- [ ] T4 Survives a TV power-cycle: with 1080p60 saved, TV off/on → Display shows 1920x1080 again within ~5 s. Repeat with 4K60 saved.
- [ ] T5 Survives a Lumen restart (Part A stop/start): comes back in the saved mode.
- [ ] T6 Mappings readable from the couch; About shows version/commit/manifest URL. Break the file: `ssh pitv 'echo bogus >> ~/.config/lumen-tv/settings.toml'` → About shows an error row, Log logs it once, the Resolution row says "not saved: …" on a switch. Restore: `ssh pitv 'sed -i "/^bogus$/d" ~/.config/lumen-tv/settings.toml'`.
- [ ] T7 Bench (Pi cooled): Settings → Toggles → Run benchmark at 1080p and 4K, Settings open and at Home: ≥ 59 fps, < 2% slow.

Part C, boot switch (Laith's sudo, on the Pi):
- [x] T8 (2026-10-24, run by Claude with Laith's cached sudo) `sh ~/lumen-tv/session/install.sh` (dry run, read it), then `sudo sh ~/lumen-tv/session/install.sh --apply` (boot default unchanged).
- [x] T9 (2026-10-24) `sudo sh ~/lumen-tv/session/install.sh --apply --quiet-boot --set-default`, then reboot: came up on `lumen.service`, Kodi inactive, tiles end with Power. Backup: `/boot/firmware/cmdline.txt.lumen-backup`.
- [ ] T10 **Gate part 1:** unplug/replug 3×: lands on Lumen with no console text or desktop after the splash. Note pointer, background flash, text.
- [ ] T11 Rescue: `ssh pitv 'pkill -SEGV -x lumen-tv'` 3× (wait for Lumen to come back between) → Kodi comes up. Then `sudo systemctl reset-failed lumen.service && sudo systemctl start lumen.service`.
- [ ] T12 **Gate part 2 (BRIEF §11):** (a) power-on lands on Lumen; (b) remote + DualSense navigate Home, Store, Settings, every panel (Laith to confirm the DualSense counts for BRIEF's "Xbox controller"); (c) red/PS returns to Lumen from Jellyfin and Steam Link; (d) Jellyfin resumes (same PID), Steam Link closes others on launch and itself on Home; (e) a Store install (KolourPaint) appears on Home without a restart, then uninstall; (f) animations smooth; (g) README covers setup, adding an app, a controller mapping, another manifest.
- [ ] T13 DualSense first reconnect after power-off: does the first PS press register? (M4.5 G4 flake.)
- [ ] T15 Power tile (last on Home): Restart Lumen → back to Home; Reboot → lands on Lumen; Shut down → Pi off; Back on the confirm step cancels.
- [ ] T14 Visual polish walk-through: note overscan margins, text sizes, focus visuals, transitions, Store/Details/Bluetooth layouts → becomes a follow-up builder job before M5 closes.

M5 contracts (as built): mode precedence settings.toml `mode` > `LUMEN_MODE` > 1920x1080@60; store precedence settings.toml `store_url` > `LUMEN_STORE_URL` > default (`~/.config/lumen-tv/settings.toml`, written atomically by the Resolution toggle). `platform/output.rs` polls wlr-randr every 2 s and re-applies the saved mode (3 tries, then gives up until the output reappears or the target changes). `lumen.service`: `Restart=always`, `StartLimitBurst=3` in 60 s, then `OnFailure=kodi.service`. Build-round review/test reports: docs/ARCHIVE.md.

## M5.1 build round (2026-10-24/25): review + test reports
Moved from IN-PROGRESS.md. All three blocking review issues below were fixed before the Pi build.

### Review (M5.1, Job 3.1, 2026-10-24)
**Summary:** Every scope item in Jobs 1.1-2.2 is there. Mac `cargo test` gives 306 passed, the wrapper's 9 Python tests pass, `node --check cap.js` passes and the manifest is valid JSON. The browser quoting is injection-safe. The `.shipped` seeding matches all four rules. With no `keys`, the pad and remote paths behave exactly as before. Home during mirroring is reliable: the waylandsink window stays unbound and `fg` stays `App(airplay)`, so `home()` closes the GTK window and SIGTERMs, then SIGKILLs, both the wrapper and `uxplay` (pids collected before the close). There is one blocking bug: the wrapper's log reader can hold UxPlay lines back indefinitely, so "stop on the phone returns within ~6 s" (A4) can fail. There is also one `uxplay` leak path and one keymap-restore gap. Not ready for the TV until the Critical is fixed.

**Issues**
1. **Critical**: `session/lumen-airplay:94-95,157-169`. `on_output` calls `proc.stdout.readline()` once per `IO_IN` wakeup. But `proc.stdout` is a buffered TextIOWrapper, which reads about 8 KB chunks from the pipe. When UxPlay writes several lines between wakeups (its startup banner, connection setup), one wakeup pulls all of them into Python's buffer and returns only the first. The fd is then empty, so GLib doesn't fire again until UxPlay writes more, and the backlog grows with every burst. If the STOP line ends up in that backlog and UxPlay then goes quiet, `watch.mirroring` stays True. The idle path is gated on `not mirroring`, so the wrapper sits on "Mirroring" forever: A4 fails, and so does the A6-style give-up. *Fix:* start with `bufsize=0` and binary pipes. In `on_output`, call `os.read(fd, 65536)`, append to a `bytearray`, split complete lines on `b"\n"`, and feed each one, decoded with `errors="replace"`. Treat an empty read as HUP. Add a test that feeds two lines in one chunk, through a small pure `split_lines(buf, chunk)` helper.
2. **Required**: `session/lumen-airplay:94` plus `src/apps.rs:495-515`. `uxplay` outlives the wrapper whenever the wrapper dies without Lumen sending a Terminate. `process_exited` → `finish` sends no Terminate step, so nothing sweeps the tree. That happens on: an unhandled exception between `Popen` (line 94) and `Gtk.main()` (for example, `Gdk.Display.get_default()` returns None at line 108); a SIGTERM before `unix_signal_add` (line 200), because Python's default action kills it; or a SIGKILL or OOM kill of the wrapper. The orphan keeps advertising "pitv" and holds UxPlay's ports, so the next AirPlay launch's `uxplay` fails and the tile shows "uxplay exited by itself" until Lumen restarts (only the `lumen.service` cgroup kill clears it). *Fix:* (a) pass `preexec_fn` that calls `ctypes.CDLL(None).prctl(1, signal.SIGTERM)` (PR_SET_PDEATHSIG; `stdbuf` → exec `uxplay` keeps it). (b) Build the window and install the signal handlers before `Popen`. (c) Wrap everything after `Popen` in `try: ... finally: stop_uxplay()`.
3. **Required**: `src/platform/pi.rs` `Desired` / `plan` / `apply_desired`. The restart restore covers only the `REMAP` scancodes. The kernel keymap survives a Lumen crash or restart. `DESIRED` starts with `dirty: false`, so the first claim after a restart writes `plan(&[], false)` = Select and Back only. Shipped YouTube (Back only) is restored, with a spurious "expected 174" warning. An override on Up/Down/Left/Right/PlayPause, for example `PlayPause = "KEY_K"` if Y6 needs it, stays in the keymap after a crash with that app in front, and the remote's Play/Pause is dead everywhere until that app is foregrounded and left again. *Fix (structural, removes concepts):* always write the full `BUTTONS` plan. `remap_remote` already skips entries that are correct ("already"), so this costs 8 GET ioctls. That deletes `full`, `dirty`, the `always`/`over` filter and the `warn` flag (check `now` against `default`/`to` only), and fixes the restart case. Update `no_overrides_plan_is_exactly_remap` to assert that the resulting keymap equals the REMAP targets plus the rc-cec defaults.

**Nits / FYI**
- Nit, `src/input/keys_out.rs` `resolve_overrides`/`action_name`: re-declares the action list that `registry::KEY_ACTIONS` already holds. Use `KEY_ACTIONS.iter().find(...)` so the two can't drift. The comment "lowercase-free action names" is garbled.
- Nit, `src/session.rs` `sync_tiles`: three separate `match c.lc.foreground()` blocks (web flag, keys, label) are now bolted onto a function documented as "tiles + poller". Look up the foreground entry once and move the input side into `fn sync_foreground_input(c, fg_entry: Option<&Entry>)`.
- Nit, `session/lumen-airplay` `stop_uxplay`: blocks the GTK loop for up to 5 s (the window freezes). That's harmless here because Lumen has already switched away, but a one-line comment would help.
- FYI, `store/manifest.json`: the running M5 Lumen on pitv parses the Store manifest from GitHub raw with `deny_unknown_fields`. Pushing this manifest before pitv runs the new build shows `youtube` as a manifest error in the Store. Deploy first, then push.
- FYI, `keys` is accepted on `command` entries (by design), but there it only remaps the TV remote: the pad injects keys for `url` apps only. The README row says "the pad's keys and the TV remote's", so add "(pad: web apps only)".

**What works well**
- `quote_browser_arg` keeps everything single-quoted and only splices a bare `"$HOME"` between quoted halves. `$(...)`, `'` and non-leading `~` can't escape (tested), and a no-extras entry yields exactly today's line.
- The keymap last-writer-wins design is right: state is recorded on the UI thread in foreground order, and the newest record is read under the write lock. A late worker or a re-claim can't leave a stale map.
- Seeding is minimal and correct: it never overwrites, never resurrects a deleted default, writes the marker via tmp+rename, and has tests for all four cases.

**Verdict:** Patch and ship. Fix #1 (the gate blocker), #2 and #3, then run Job 4.1.

### Test Results (M5.1, Job 4.1, 2026-10-24)
**Verdict: Fail** (one step, pitv clippy, could not run: the Pi's Rust toolchain has no clippy component. Everything else passes.)

Read-only check of the review fixes before the build: `session/lumen-airplay` has the `os.read` chunked reader, `PR_SET_PDEATHSIG` and the `try/finally` stop path; `src/platform/pi.rs` no longer has the `dirty`/`full` fields.

| # | Command | Where | Exit | Result |
|---|---|---|---|---|
| 1 | `~/.cargo/bin/cargo test` | Mac | 0 | 304 passed, 0 failed, 2 ignored |
| 2 | `~/.cargo/bin/cargo clippy --all-targets` | Mac | 0 | 5 warnings, all known: `bluetooth.rs:432` (manual_div_ceil), `icons.rs:42` and `icons.rs:70` (large_enum_variant), `input/mapping.rs:296`, `store.rs:463`. No new warnings. |
| 3 | `python3 -m unittest session/test_lumen_airplay.py` | Mac | 0 | Ran 11 tests, OK |
| 4 | `sh -n session/install.sh` | Mac | 0 | no output |
| 5 | `node --check config/browser/youtube-tv/cap.js` | Mac | 0 | no output |
| 6 | `scripts/pi-build.sh` (rsync + `cargo build --release`) | Mac → pitv | 0 | `Finished release profile [optimized] target(s) in 2m 41s`. No compile errors or warnings. |
| 7 | `ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo test --release'` | pitv | 0 | `test result: ok. 312 passed; 0 failed; 2 ignored` (Linux-only tests included) |
| 8 | `ssh pitv 'cd lumen-tv && ~/.cargo/bin/cargo clippy --release --all-targets'` | pitv | **1** | `error: 'cargo-clippy' is not installed for the toolchain 'stable-aarch64-unknown-linux-gnu'.` / `help: run \`rustup component add clippy\` to install it`. Not run: needs a toolchain component install on pitv (Laith's call). |
| 9 | `ssh pitv 'cd lumen-tv && python3 -m unittest session/test_lumen_airplay.py'` | pitv | 0 | Ran 11 tests, OK |

Not run, per the job: `lumen.service`, `lumen-test.service`, `uxplay.service` and `kodi.service` were not touched. No sudo. No code changed. The Pi binary was rebuilt in `~/lumen-tv/target/release/` but nothing was restarted, so the running Lumen is still the previous build.

Open: pitv clippy needs `rustup component add clippy` on the Pi before it can gate. Then re-run step 8 (or accept the Mac clippy result, which is the same source).

## M5.1 TV checklist (2026-10-25, all pass)
Y1-Y7 pass (Laith; codec avc1; YouTube PID 1527 kept through 8 Home round trips). Y8 Home bench with YouTube in the background: motion 60 fps / 0% slow, idle 60.1 / 0.6%. A1-A2 pass (iPhone 17 connected, video window app_id `uxplay` fullscreen, sound). A3: longest logged mirror 4 min 4 s, accepted by Laith as smooth. A4 phone stop 11:51:37 → Home 11:51:43 (6 s). A5 red mid-mirror 11:50:40 and PS mid-mirror 11:57:58, both close in 1 s, no uxplay left. A6 no client 11:51:48 → gave up 11:53:49 (121 s). A7 uxplay.service/kodi.service never active. Pi 66-72 °C during the test.
Pre-TV Pi checks: V1 packages (uxplay 1.71.1, plugins bad/good/libav, python3-gi, GTK 3, avahi active, nothing to install); V2 Store manifest 6 apps / 0 errors; V3 waylandsink has `fullscreen` (read from the .so; no Gst gir on the Pi); V4 wrapper compiles + executable; V5 headless Chromium with UA candidate A gets "YouTube on TV" (leanback `ytlr-*`) and the extension's `data-lumen-cap="1"` (so `--load-extension` works on Chromium 155; without the UA the page is empty); V6 `youtube.toml`, `airplay.toml`, `.shipped` seeded, 0 registry errors. Pi clippy not run (no clippy component on pitv; Mac clippy clean).


## M5.3 Kodi inventory (2026-10-25, Job 1.4, read-only)

### Commands run (all read-only via `ssh pitv`, no sudo)
`dpkg -l | grep`, `apt-mark showmanual`, `dpkg-query -W`, `apt-cache depends/rdepends --installed`, `apt-get -s remove` (simulation, 3 variants), `systemctl is-enabled/is-active/cat/list-dependencies`, `ls`/`grep -l` on /etc/systemd/system and /etc/sudoers.d, `du -sh`, `find` (saves), `ls -la`, `cat ~/.local/bin/kodi-tile`. Plus local reads of Pi-tv/CLAUDE.md, config/, lumen-tv/config/apps, session/. Nothing was started, stopped, killed, written or deleted.

### 1. Packages
Installed (dpkg): kodi, kodi-bin, kodi-data, kodi-repository-kodi (all 3:21.3+dfsg-1+rpt3), kodi-game-libretro 21.0.7-1, kodi-inputstream-adaptive 21.5.9+ds-1, kodi-visualization-spectrum 21.0.2+ds-1, libretro-desmume 0.9.11+git20160819, libretro-gambatte 0.5.0+git20160522, libretro-mgba 0.10.5, steamlink 1.0.16, uxplay 1.71.1-1, gstreamer1.0-{alsa,gl,libav,plugins-bad,plugins-base,plugins-good,x} 1.26.2.

apt-mark showmanual: gstreamer1.0-alsa, gstreamer1.0-plugins-bad, gstreamer1.0-plugins-good, kodi, kodi-game-libretro, kodi-inputstream-adaptive, libretro-desmume, libretro-gambatte, libretro-mgba, steamlink, uxplay.

| Group | Packages | Evidence |
|---|---|---|
| (a) Kodi's own, manual | kodi, kodi-game-libretro, kodi-inputstream-adaptive | kodi Depends kodi-bin, kodi-data; Recommends kodi-repository-kodi, kodi-visualization-spectrum. The other two have no rdepends. |
| (a) Kodi's own, auto (stay unless Laith autoremoves) | kodi-bin, kodi-data, kodi-repository-kodi, kodi-visualization-spectrum, ~40 libs | listed by apt as "no longer required" |
| (b) Kodi-only but generic | libretro-mgba, libretro-gambatte, libretro-desmume | symlinked from ~/.kodi/addons/game.libretro.*; mgba Depends `retroarch \| libretro-frontend` |
| (c) Shared | uxplay (Lumen AirPlay tile), gstreamer1.0-*, steamlink | no Kodi dependency |

### 2. Proposed `[kodi]` allowlist
- install: kodi, kodi-game-libretro, kodi-inputstream-adaptive, libretro-mgba, libretro-gambatte, libretro-desmume
- remove (recommended): the same six (see gotcha)
- remove (job-literal): kodi, kodi-game-libretro, kodi-inputstream-adaptive. Not viable as-is: it pulls in RetroArch + Qt5.
- Allowlist comment: apt `remove` keeps ~/.kodi and ~/roms (no purge, no autoremove).

### `apt-get -s remove -- kodi kodi-game-libretro kodi-inputstream-adaptive` (verbatim, lists only)
```
The following packages were automatically installed and are no longer required:
  kodi-bin kodi-data kodi-repository-kodi libavdevice61 libcdio-cdda2t64
  libcdio-paranoia2t64 libcdio19t64 libcec7 libcrossguid0 libdate-tz3
  libfstrcmp0 libimagequant0 libiso9660-12 libjs-iscroll libjs-jquery
  libkissfft-float131 libldb2 liblirc-client0t64 liblmdb0 libmariadb3
  libmicrohttpd12t64 libnfs14 libp8-platform2 libraqm0 libshairplay0
  libsmbclient0 libspdlog1.15 libtdb1 libtevent0t64 libtinyxml2-11
  libva-wayland2 libwayland-client++1 libwayland-cursor++1 libwayland-egl++1
  libwebm1 mariadb-common mysql-common python3-olefile python3-pil samba-libs
Use 'apt autoremove' to remove them.
The following additional packages will be installed:
  fonts-noto-extra gamemode-daemon libgamemode0 libqt5core5t64 libqt5dbus5t64
  libqt5gui5t64 libqt5network5t64 libqt5qml5 libqt5qmlmodels5 libqt5quick5
  libqt5svg5 libqt5waylandclient5 libqt5waylandcompositor5 libqt5widgets5t64
  libretro-core-info pkexec qt5-gtk-platformtheme qttranslations5-l10n
  qtwayland5 retroarch retroarch-assets
The following packages will be REMOVED:
  kodi kodi-game-libretro kodi-inputstream-adaptive
  kodi-visualization-spectrum
0 upgraded, 21 newly installed, 4 to remove and 0 not upgraded.
```
kodi-visualization-spectrum goes because it depends on kodi's API (auto-installed).

### Simulation with the 3 cores added (recommended)
`apt-get -s remove -- kodi kodi-game-libretro kodi-inputstream-adaptive libretro-mgba libretro-gambatte libretro-desmume`
Result: `0 upgraded, 0 newly installed, 7 to remove` (kodi-visualization-spectrum, the three cores, kodi-game-libretro, kodi-inputstream-adaptive, kodi). Nothing shared is removed.

### 3. Units, boot, autologin
| Unit | enabled | active |
|---|---|---|
| kodi | disabled | inactive |
| steamlink | static | inactive |
| uxplay | static | inactive |
| lumen | enabled (multi-user.target.wants) | active |
| lumen-test | static | inactive |
| getty@tty1 | enabled (getty.target.wants) | inactive |
| autovt@tty1 | enabled | inactive |

`systemctl cat getty@tty1` shows only the stock /usr/lib/systemd/system/getty@.service (`agetty -o '-- \u' --noreset --noclear - ${TERM}`). Neither /etc/systemd/system/getty@tty1.service.d nor getty@.service.d exists. **No autologin.**

Boot without lumen: getty.target pulls getty@tty1, so a disabled lumen leaves a plain login prompt. With Lumen enabled, `OnFailure=kodi.service` sends a failed Lumen to Kodi (kodi.service Conflicts= getty@tty1), not to a shell. Once Kodi is gone that rescue is dead (the M6 rescue-to-getty change), and `install.sh --revert` (`systemctl enable kodi.service`) fails.

`grep -l kodi` hits: kodi.service, lumen.service, lumen-test.service, steamlink.service, uxplay.service. sudoers.d (010_at-export, 010_dpkg-threads, 010_global-tty, 010_proxy, lumen, README, steamlink, uxplay) is root 0440 and unreadable without root; contents of steamlink/uxplay are known from the repo.

### 4. Steam Link
Lumen tile `config/apps/steamlink.toml`: `launch = "steamlink"`, process `~/.local/share/SteamLink/bin/shell`. The `steamlink` package has no Kodi dependency. Kodi-era `steamlink.service` is the only link to Kodi (ExecStopPost starts it).

### 5. Data (sizes only)
~/.kodi 240M (addons 203M, userdata/Thumbnails 26M, temp 120K); ~/.kodi-tile 132K (symlinks to ~/.kodi: addons, media, saves, system, userdata/*); ~/.kodi-backup-20261013 238M; ~/roms 1.1G; ~/.local/share/SteamLink 130M.
ROMs: gb/Pokemon Yellow; gba/Pokemon Emerald; nds/Black, Black 2, Platinum, SoulSilver.

### Saves location
- `~/.kodi/saves/<ROM filename>/`: in-game saves (only `Pokemon - Platinum Version (USA).nds/`, empty)
- `~/.kodi/userdata/Savestates/game.libretro.desmume/`: save states (empty)
- `~/.kodi/userdata/addon_data/game.libretro.desmume/save/`: add-on save dir (empty)
- `gamesgeneral.enableautosave` = true. gambatte and mgba have no save folders yet.
No .sav/.srm/.dsv/.state file exists anywhere (`find / -xdev`). The paths come from directories Kodi created on first play, not from a written save. Nothing to import into RetroArch today. Back up ~/.kodi/saves and userdata/Savestates before any Kodi cleanup.

### 6. Dead-file table (K5; proposals only, nothing removed)
| File | Used by | Lumen-side use? | Recommendation |
|---|---|---|---|
| /etc/systemd/system/kodi.service (repo config/kodi.service) | old boot session; lumen.service OnFailure; `--revert` | yes, as rescue until M6 | Keep until the rescue becomes getty; then remove |
| /etc/systemd/system/steamlink.service (config/) | Kodi favourite "Steam Link"; ExecStopPost starts Kodi | no (Lumen runs `steamlink` directly) | Remove; then drop it from lumen.service Conflicts= |
| /etc/systemd/system/uxplay.service (config/) | Kodi favourite "Screen Mirror" | no (Lumen has `session/lumen-airplay`) | Remove |
| /etc/sudoers.d/steamlink, uxplay (config/sudoers-*) | the two start scripts | no | Remove after the units |
| ~/.local/bin/start-steamlink.sh, start-mirror.sh (config/) | Kodi favourites | no | Remove |
| ~/.local/bin/airplay-mirror (config/) | uxplay.service | no | Remove |
| ~/.local/bin/kodi-tile (lumen-tv/config/kodi-tile) | Lumen's Kodi tile (`config/apps/kodi.toml`) | yes, until Kodi is uninstalled | Keep until M5.3 test passes |
| ~/.config/lumen-tv/apps/kodi.toml, kodi.toml.bak | Lumen registry | yes | Leave to M5.3 Store logic; .bak is stale |
| config/favourites.xml, sources.xml, skinshortcuts-mainmenu.DATA.xml | Kodi | no | Keep as reinstall references (K5) |
| config/kodi-games/ | Kodi RetroPlayer add-ons | no | Keep (reinstall reference) |
| ~/.kodi, ~/.kodi-tile, ~/.kodi-backup-20261013, ~/roms, saves | Kodi data | tile links into ~/.kodi | NEVER touch |
| config/nftables.conf, wireplumber-51-pitv-hdmi.conf | firewall / audio | yes | Keep |

### Removal commands for Laith (not run; after the M5.3 TV test and his approval)
```
sudo rm /etc/systemd/system/steamlink.service /etc/systemd/system/uxplay.service
sudo rm /etc/sudoers.d/steamlink /etc/sudoers.d/uxplay
sudo systemctl daemon-reload
sudo visudo -c
rm ~/.local/bin/start-steamlink.sh ~/.local/bin/start-mirror.sh ~/.local/bin/airplay-mirror ~/.local/bin/kodi-tile
# later, once Kodi is gone and the rescue is getty:
sudo rm /etc/systemd/system/kodi.service && sudo systemctl daemon-reload
```
Repo-side equivalents (config/steamlink.service, uxplay.service, sudoers-*, start-*.sh, airplay-mirror) go in a follow-up job after approval.


## M5.3 Phase 2 security review (2026-10-25, Job 2.0a; all items fixed the same day)
**M5.3 review** (Job 2.0a, Phase 1; all HIGH/MEDIUM/LOW items fixed 2026-10-25, now helper 25 + 9, lumen-tv 324 pass; `cargo test --workspace` 320 + helper 23 + 7 pass on the Mac)
Summary: the core guarantees hold. Packages come only from the root-owned allowlist, which is re-read and permission-checked per request. Names are validated (no leading `-`) and always follow `--`. The env is cleared, no shell runs, the PIN never reaches a reply or log (serde errors are classified), the lockout is persisted and mutex-serialised, and store.rs never runs a manifest string for a root app. The issues are apt doing more than the allowlist says, one wedge, and UI-thread blocking. Verdict: **Patch and ship**. Fix HIGH/MEDIUM before Phase 3 closes.
- **HIGH (1.1) `helper/src/apt.rs:26` `args()`: `-y` lets apt go beyond the allowlist.** `remove` can install packages (the inventory's retroarch + Qt case, 21 pkgs) or cascade-remove reverse deps, and `install` can remove conflicting packages. D7/Gate 8 ("removes exactly the listed packages") is not enforced. Fix: add `--no-remove` to install. Before a remove, run the same argv with `-s` and refuse (`failed`) if the simulation has any `Inst ` line or a `Remv <pkg>` not in the entry's `remove`. Add a fake-apt test for both. Consequence: `[kodi]` remove must list `kodi-visualization-spectrum` (removed as a dependent) plus the 3 cores (gotcha above).
- **MEDIUM (1.1) `helper/src/server.rs:125-143` `handle()`: no write timeout.** A client that sends a valid request and stops reading blocks `writeln!` inside the apt loop. The apt pipe then fills and apt stalls. The 15-min kill is never reached (it is checked in the same loop), `busy` never clears and the daemon never idles. Fix: `stream.set_write_timeout(Some(10 s))`; the existing `broken` flag then drops output and apt finishes.
- **MEDIUM (1.1) `apt.rs:19` COMMON: dpkg conffile prompts fail.** With stdin `/dev/null` they fail with "end of file on stdin at conffile prompt". This bites reinstalling Kodi after `remove` (conffiles kept) and any upgrade. Fix: add `-o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold` and update the argv tests and the plan's apt shape.
- **MEDIUM (1.2) `src/store.rs:441` `run_job`: blocking `helper.status()` on the UI thread.** `storefeed::run` calls it for every root app; it can block up to 3 s on read, connect has no timeout at all (spec: 3 s), and status is fetched again in `run_job_blocking`. Fix: pass the status storefeed already fetched on its worker (`run_job(.., status: Option<&RootStatus>)`) for the sync check, and keep the worker-side re-check.
- **MEDIUM (1.2) structural, `store.rs:487-547`: the root path is a conditional inside the user-step loop.** The display text `root_command()` sits in the same `steps` Vec that `run_step` hands to `sh -c`. Only the `if let Some(pin) … continue` ordering keeps it from being executed. Fix: split out `run_root(helper, op, id, pin, on_event)`, return before the loop, share the entry/receipt tail as `finish()`. Also forward the helper's real `Step` events as lines (today they are dropped, so the screen omits the `apt-get update` step).
- **LOW (1.2) flaky `installer.rs:213` `stale_socket_file_is_unavailable`.** Reproduced: 2/8 failures with `cargo test -p lumen-tv -- store:: installer::`, 0/12 with `installer::` alone. Cause: on macOS std sets FD_CLOEXEC after `socket()` (non-atomic). A store test spawning `sh` in that window inherits the listener, so after `drop()` it still listens: connect succeeds, the 3 s read times out, and `note` is Some. Linux uses atomic SOCK_CLOEXEC, so it is not a product bug. Fix: assert `note.is_none()` only `#[cfg(target_os = "linux")]` (always assert `!available`), with a comment.
- **LOW (1.1) `main.rs:63`: fd 3 from systemd has no FD_CLOEXEC.** apt, dpkg and every maintainer script inherit the listening socket. Fix: `libc::fcntl(3, F_SETFD, FD_CLOEXEC)` after the LISTEN_FDS check.
- **LOW (1.1) `proto.rs:51` + `server.rs:147`: unvalidated client `id` is logged with `{}`.** Newlines or a 4 KiB id forge journal lines. Fix: `allowlist::valid_id` in `parse_request`, and log `{:?}`.
- **LOW (1.2) `installer.rs:146`: status reasons are lost.** A `not_allowed`/`error` status (unsafe allowlist, bad attempts file) becomes "unexpected reply". EACCES (Lumen started before joining `lumen-helper`) gets a raw note. Fix: map both into `note`, with EACCES → "restart Lumen to pick up the lumen-helper group" (for 3.1's mode 2 text).
- **LOW (1.1) hardening bundle.** Reject `--apt/--allowlist/--state-dir/--socket` outside `--insecure-test-mode`. Clamp a far-future `locked_until` on load (`pin.rs:49`; Pi 5 has no RTC battery, and a backward clock jump keeps it locked beyond 1 h while replying ≤ 3600). The PIN file is `pin.hash`, but MILESTONES/Gate 6 say `/var/lib/lumen-helper/pin`: align one.
- FYI: `ui/pinpad.slint` compiles. It was imported into a scratch slint-build 1.18.1 crate with no warnings, `ui/app.slint` untouched. The `⌫` glyph needs a system font that has U+232B on pitv; check at the TV, with "Del" as the fallback. A 4-digit PIN allows about 24 guesses/day once at the 1 h cap, so ~200 days on average for a laith-level attacker (D6); suggest 6 digits. The storefeed `None` PIN stub is 3.1's.


## M5.3 TV checklist (2026-10-26, all 12 pass; Laith at the TV)
1. Wrong PIN: "wrong PIN", nothing installed. 5 bad_pin (20:51:51-20:52:14): on-screen lock countdown. The lock still held after two `systemctl restart lumen-helper.socket` (the restarted helper reported the persisted lock).
2. Right PIN installed `root-test`: apt history `install -- sl`, progress lines on screen, tile appeared live.
3. Uninstall with the PIN: `remove -- sl`, tile gone.
4. Tampered manifest `~/m53-tampered.json` (via settings.toml `store_url`): `root-evil` (id not on the allowlist) showed the Store's "run it over SSH" fallback, no PIN pad, nothing sent. A raw request to the helper socket for `root-evil` (run by Laith) logged `Install "root-evil" -> not_allowed` (the allowlist is checked before the PIN, so no PIN attempt used). `root-test` with `vim` added in the manifest installed only `sl` (apt `install -- sl`); vim never installed. Laith decided (option a, 2026-10-26): the pass condition is "the manifest cannot add packages or ids" (the helper takes only an id and uses the allowlist's packages; Lumen does not refuse a mismatched list outright). Gate wording updated in MILESTONES.
5. Journal/apt history: `/usr/bin/apt-get -y -o DPkg::Lock::Timeout=120 ... --no-install-recommends --no-remove install -- <pkgs>` and `remove -- <pkgs>`.
6. Allowlist and binary root:root, not group/other-writable; `/var/lib/lumen-helper` root 0700 (PIN file unreadable to laith); socket root:lumen-helper 0660; the helper logs "idle, exiting" 60 s after the last request.
7. A connect to `/run/lumen-helper.sock` as user `nobody` (python3 run as nobody; socat isn't installed on pitv): PermissionError Errno 13.
8. Kodi uninstall from the Store with the PIN removed exactly the 8 allowlisted packages (kodi, kodi-game-libretro, kodi-inputstream-adaptive, libretro-mgba, libretro-gambatte, libretro-desmume, kodi-repository-kodi, kodi-visualization-spectrum); kodi-bin + kodi-data remain (expected). `du` ~/.kodi 245140 / ~/.kodi-tile 132 / ~/roms 1066016 identical before/after. Tile gone live; Restart Lumen did not bring it back. `~/.config/lumen-tv/apps/kodi.toml.bak` remains (not loaded).
9. Kodi install from the Store: 7 packages, tile live, launched fullscreen=true, red -> Home terminated it. Uninstalled again, so pitv ends without Kodi (only kodi-bin/kodi-data). ~/.kodi grew 16 KB and ~/.kodi-tile 8 KB from Kodi's own writes during launch; roms unchanged.
10. Rapid Restart Lumen -> `start-limit-hit` -> `OnFailure=` -> getty@tty1 login prompt, no autologin override; SSH fine. `reset-failed` + `start` brought Lumen back and stopped getty.
11. Steam Link streamed and Home closed it (exit 0). AirPlay mirrored from the iPhone and returned to Lumen after the phone stopped (harmless `GStreamer-Wayland-CRITICAL gst_wl_window_ensure_fullscreen` assertions in the log).
12. `cargo test --workspace`: Mac helper 57 + 9, lumen-tv 348 / 2 ignored; pitv 57 + 9 + 356 / 2 ignored (3 runs), release build no warnings.

Gate setup (2026-10-26): pitv's `~/.config/lumen-tv/settings.toml` got `store_url = "/home/laith/lumen-tv/store/manifest.json"` because the public GitHub manifest lacked the kodi/root-test entries (backup `settings.toml.m53bak`). Remove that line after `store/manifest.json` is pushed.

### M5.3 build phases (rotated from IN-PROGRESS Active Plan, 2026-10-26)
Phases 1-2 (2026-10-25): `lumen-helper` crate, Lumen client + Store job path, PIN pad, Kodi inventory, security review (above). Phase 3: Store PIN flow, session files, installer, allowlist, manifest entries, Kodi out of the defaults, console rescue in the units. Phases 4-5 (2026-10-26): review (no critical; Step-after-PIN moved into `store::run_root`, relative `--store-check` paths, install.sh helper preflight first, `cmp` on units, sha256 of the root binary), coverage (helper 25 -> 57 tests), docs. K6 gained `kodi-repository-kodi` (Laith, 2026-10-26; pitv sim: 8 removed, 0 installed). Pi-only bug fixed: helper RST on oversized requests (half-close + bounded drain). Before-snapshot for check 8: 10 kodi/libretro pkgs installed; du ~/.kodi 245140, ~/.kodi-tile 132, ~/roms 1066016.

## M5.5 Phase 6b/6c — Spotify Connect review + test results (2026-10-27, moved from IN-PROGRESS)
All findings were fixed before 6c (curl stall limits + TimeoutStartSec, flag/arch checks, uninstall stops the update unit, `type` rejected on tile:false, notes wording).

### Review — Phase 6a Spotify Connect (Job 6b.1, 2026-10-27)
**Summary:** Ready to test after one fix. All scope items of 6a.1-6a.3 are in. Key handling is sound: the key goes into a temp file made under `umask 077` in the 0700 key dir, written by the `printf` builtin, then `mv`. It never reaches stdout, stderr or any argv the script controls; the only exception is the accepted `soloist -k` in the unit. The tty echo comes back on every exit path (checked by hand under a pty: Ctrl-C during the prompt restored echo, exit 130, no key file left). The download is https-only (`--proto '=https'` also covers redirects), checked for ELF magic and e_machine 0xb7, and renamed in from the same directory. Uninstall is idempotent. The unit keeps `127.0.0.1:9090` and `RestartPreventExitStatus=10`, and `ConditionPathExists` stops a restart loop when there is no key. `tile: false` uses one predicate (`store::is_installed`), and tiled apps keep their exact old path in `check`, `commit`, the feed and the loop. The `validate_identity` split only changes which error is reported first when an entry has several. `dash -n` is clean and the 31 stubbed tests pass. The notes URL and the CDN URL both return 200.

**Issues**
1. **Required**: `session/lumen-soloist:169` (`fetch`) and `:115-124` (`unit_update_service`). curl has `--connect-timeout` but no `--max-time` or `--speed-limit`, and the update unit is `Type=oneshot`, whose default `TimeoutStartSec` is infinity. If a weekly download stalls, `soloist-update.service` stays "activating" forever, the timer can't start it again, and the binary expires silently at day 90 (exit 10, no music). Store steps are safe: they have a 15-minute limit and the group gets killed. Fix: add `--speed-limit 1024 --speed-time 60` (or `--max-time 600`) to curl, and `TimeoutStartSec=15min` to the update service. Add one test that asserts both are present.
2. **Nit**: `src/store.rs:29-32,75-78`. A `type` key on a `tile: false` entry is accepted without a word and then dropped from the receipt. The contract says each forbidden key must give a parse error. Fix: add `"type"` to `TILE_ONLY_KEYS` and delete the separate `o.remove("type")` in `to_value`. The loop already strips it.
3. **Nit**: `session/lumen-soloist:45-53,202-211,259`. The CLI accepts too much: `--force` and `--purge` are taken with any command (`install --purge` is silently ignored), `need_arch` accepts `arm64`, so a non-dry run on the Mac would download Spotify's binary into the Mac's `~/.local/lib`, and `update` on a box without the units installs a bare binary. Fix: reject a flag outside its command, accept only `aarch64`, and in `cmd_update` add `[ -f "$UNITDIR/$SVC" ] || die "not installed"`.
4. **Nit**: `session/lumen-soloist:292-308`. `uninstall` doesn't stop `soloist-update.service`. An update that is running (or hung, see 1) can run `swap_in` → `mkdir -p "$LIB"` after the `rm -rf` and leave a binary behind after a successful uninstall. Fix: `sc stop "$UPD_SVC" || true` before the `rm`s. Also, `--purge` deletes `~/.cache/soloist` but its message names only the key and the state: name the cache too.
5. **Nit**: `store/manifest.json` (spotify-connect `notes`). "This is not an unofficial hack, but Spotify's terms may not cover every tool built around it" reads as contradictory on the details screen. Fix: "Soloist is Spotify's own software, used here under Spotify's terms; Spotify can change or withdraw it at any time. Use at your own risk."
6. **FYI**: `systemctl --user status soloist.service` lists the CGroup command line, so it prints the key. The script's `status` uses `is-active` only (good). Docs (6b.3) and the SC checks must never suggest `status` or `journalctl -u ... -o verbose`. SC4 expects "unchanged, no restart", but the script prints "Soloist is already up to date; nothing to do": align one of them. If an entry ever switches between `tile: true` and `tile: false` while installed, `is_installed` and `commit` follow the manifest, not the receipt, which would leave a TOML orphaned. No entry does this today. If it matters, take `tile` from the receipt when one exists.

**What works well**
- The binary swap: `cp` to `$LIB/.soloist.new` and then `mv` in the same directory, with the ELF/e_machine check done in the temp dir first. A failed download, unpack or check leaves the old install alone, and tests cover that.
- `store::is_installed` is a single predicate used by `check`, the feed and the loop, and `commit` returns early for `tile: false`, so the tiled path is byte-for-byte unchanged.
- The key-leak tests run the real script under dash with argv-logging stubs and assert that the secret appears in none of stdout, stderr or the stub logs.

**Verdict:** Patch and ship. Fix 1 (`session/lumen-soloist`, Job 6a.2's files) before 6c. The Nits can go in the same pass.

### Test Results — Phase 6c (Job 6c.1, 2026-10-27)
**Verdict: PASS.** No code changed, no commits, nothing installed, soloist.service and lumen.service untouched, Lumen not restarted. Logs are in the tester's scratchpad.

| # | Where | Command | Exit | Result |
|---|---|---|---|---|
| 1 | Mac | `cargo test --workspace` | 0 | lumen-tv 372 passed, 0 failed, 2 ignored; helper 57 + 9 passed |
| 2 | Mac | `cargo run -q -- --store-check store/manifest.json` | 0 | 16 apps, 0 errors; spotify-connect `tile: no` |
| 3 | Mac | `cargo run -q -- --store-check store/fixtures/test-manifest.json` | 0 | 4 apps, 0 errors; notile-test `tile: no` |
| 4 | Mac | `python3 -m unittest session/test_lumen_soloist.py` | 0 | Ran 37 tests, OK |
| 5 | Mac | `python3 -m unittest session/test_lumen_airplay.py` | 0 | Ran 11 tests, OK |
| 6 | Mac | `dash -n session/lumen-soloist` | 0 | no output |
| 7 | Pi | `sh scripts/pi-build.sh` (rsync + release build on pitv) | 0 | `Finished release profile [optimized] in 2m 36s` |
| 8 | pitv | `cargo test --release --workspace` | 0 | lumen-tv 380 passed, 0 failed, 2 ignored; helper 57 + 9 passed |
| 9 | pitv | `lumen-tv --store-check /home/laith/lumen-tv/store/manifest.json` | 0 | 16 apps, 0 errors |
| 10 | pitv | `python3 -m unittest session/test_lumen_soloist.py` | 0 | Ran 37 tests, OK |
| 11 | pitv | `lumen-tv --store-loop --dry-run --source .../store/manifest.json` | 0 | 7 to run, 9 skipped; `spotify-connect  SKIP  no window: background service, checked at the TV` |
| 12 | pitv | `sh ~/lumen-tv/session/lumen-soloist --dry-run install` | 0 | output below |
| 13 | pitv | `sh ~/lumen-tv/session/lumen-soloist --dry-run uninstall` | 0 | output below |

Not run: `test_lumen_airplay.py` on pitv (not in the job). The Mac `airplay` run is 11 tests, OK.

**Notes for Laith before 6d:**
- The Mac and Pi lumen-tv test counts differ (372 vs 380). Same tree, so the 8 extra are probably Pi-only (`cfg`-gated) tests. Not a failure.
- The install dry run writes `~/.config/systemd/user/soloist.service`, the same path as the hand-installed unit. It then runs `enable` and starts Soloist only when the binary or unit changed. The uninstall dry run would stop `soloist.service` and delete the binary. Neither runs in 6c.
- Uninstall keeps the key and login state, as Q1 says. `--purge` removes both.

**`sh ~/lumen-tv/session/lumen-soloist --dry-run install` (pitv, exit 0):**
```
[dry-run] download https://soloist-builds.spotifycdn.com/soloist_release_arm64.tar.gz (curl --proto '=https' -fL) into a temp dir, unpack, verify an aarch64 ELF
[dry-run] install the binary and its notices into /home/laith/.local/lib/soloist (existing install kept until the swap)
[dry-run] write /home/laith/.config/systemd/user/soloist.service:
    | [Unit]
    | Description=Spotify Soloist (Spotify Connect receiver, named after this box)
    | After=pipewire.service pipewire-pulse.service wireplumber.service network-online.target
    | Wants=pipewire.service
    | # Without a key Soloist can't start: don't restart-loop, wait for `lumen-soloist set-key`.
    | ConditionPathExists=%h/.config/soloist/key
    | 
    | [Service]
    | # Soloist only announces the addresses it sees at start, and a user unit can't wait for
    | # network-online.target: wait (max 60 s) for an IPv4 address (wlan0 if it exists, else any).
    | ExecStartPre=/bin/sh -c 'for i in $(seq 60); do if [ -d /sys/class/net/wlan0 ]; then ip -4 -o addr show wlan0 | grep -q inet && exit 0; else ip -4 -o addr show scope global | grep -q inet && exit 0; fi; sleep 1; done; exit 0'
    | # Device name = hostname (%H). WebSocket API on loopback (lumen-karaoke connects there).
    | # Soloist takes the key only via -k (no env var or key file option), so it comes from the 0600 key file.
    | # Exit code 10 = build expired: the weekly soloist-update.timer re-downloads it.
    | ExecStart=/bin/sh -c 'exec %h/.local/lib/soloist/soloist -n %H -z 300 -w 127.0.0.1:9090 -k "$(cat %h/.config/soloist/key)"'
    | Restart=on-failure
    | RestartSec=5
    | RestartPreventExitStatus=10
    | 
    | [Install]
    | WantedBy=default.target
[dry-run] write /home/laith/.config/systemd/user/soloist-update.service:
    | [Unit]
    | Description=Update the Spotify Soloist binary (Lumen)
    | 
    | [Service]
    | Type=oneshot
    | TimeoutStartSec=15min
    | ExecStart=/bin/sh "/home/laith/lumen-tv/session/lumen-soloist" update
[dry-run] write /home/laith/.config/systemd/user/soloist-update.timer:
    | [Unit]
    | Description=Weekly Spotify Soloist update check (builds expire after 90 days)
    | 
    | [Timer]
    | OnCalendar=weekly
    | RandomizedDelaySec=6h
    | Persistent=true
    | 
    | [Install]
    | WantedBy=timers.target
[dry-run] systemctl --user daemon-reload
[dry-run] systemctl --user enable --now soloist-update.timer
[dry-run] systemctl --user enable soloist.service
[dry-run] if /home/laith/.config/soloist/key exists: start soloist.service (restart only if it was active and the binary or unit changed)
[dry-run] otherwise: print 'waiting for a key (run set-key over SSH)'
```

**`sh ~/lumen-tv/session/lumen-soloist --dry-run uninstall` (pitv, exit 0):**
```
[dry-run] systemctl --user stop soloist.service
[dry-run] systemctl --user stop soloist-update.timer
[dry-run] systemctl --user stop soloist-update.service
[dry-run] systemctl --user disable soloist.service soloist-update.timer
[dry-run] rm -f /home/laith/.config/systemd/user/soloist.service /home/laith/.config/systemd/user/soloist-update.service /home/laith/.config/systemd/user/soloist-update.timer
[dry-run] rm -f /home/laith/.config/systemd/user/default.target.wants/soloist.service /home/laith/.config/systemd/user/timers.target.wants/soloist-update.timer
[dry-run] rm -rf /home/laith/.local/lib/soloist
[dry-run] systemctl --user daemon-reload
[dry-run] systemctl --user reset-failed soloist.service soloist-update.service soloist-update.timer
kept your key (/home/laith/.config/soloist/key) and login state (/home/laith/.local/share/soloist); add --purge to remove them
uninstalled Soloist
```

## M5.5 TV Gate (2026-10-26/28, all pass; Laith at the TV, Claude over SSH)
Result line in MILESTONES.md. Spike report follows below this checklist.

**New entries (8, all pass):** each installed from the Store with the remote, tile live without a restart, fullscreen, main action reached, red = Home, Store uninstall.
- **Spotify (web):** open.spotify.com in kiosk Chromium + `libwidevinecdm0` through the PIN root helper. Playback, Connect device from the phone, lyrics via the remote, red keeps the music playing. `config/browser/spotify-tv/` remote navigation works. Laith's report that Left/Right on the player bar seeked the track was fixed in `remote.js` (sliders not focusable, arrows always move focus) and he confirmed it. Uninstall also removed `libwidevinecdm0`; reinstall put it back through the PIN.
- **Spotify Connect (Soloist, `session/lumen-soloist`, `tile: false`):** SC1, SC3, SC4, SC5 PASS at the TV. SC2 (`set-key`) not run live because the key pre-existed; covered by script tests. SC6 = the bench below. Details of 6b/6c in the section above.
- **RetroArch (Flatpak via Xwayland):** DualSense works out of the box; Laith played a GB, a GBA and a DS game.
- **Moonlight, chiaki-ng:** install, tile, fullscreen, buttons, red = Home and uninstall pass. **Streaming itself NOT tested** (Laith has no host/console).
- **VLC, Audiobookshelf, Twitch:** all checks pass.
- **Store section headers** never take focus (pass).

**Dropped conditionals (removed from `store/manifest.json`; manifest now 13 apps, `--store-check` 0 errors):**
- YouTube Music: with the GoogleTV UA (claims Chrome 11) the site says "not optimized for your browser"; with Chromium's own UA it loads and plays, but the desktop page can't be driven with the remote alone.
- Pluto TV: black screen with the GoogleTV UA. Tubi: "use a supported browser" with the GoogleTV UA. Neither retried with Chromium's UA.

**Uninstall checks over SSH** (Twitch, YouTube Music, Pluto, Tubi, Spotify, Moonlight, chiaki-ng, VLC): TOML, receipt and Flatpak/package all removed. Flatpak user data stays in `~/.var/app/<id>` (Moonlight, Chiaki4deck, VLC), like the old kolourpaint leftover; acceptable (the Gate asks for package, TOML, tile).

**Automated `--store-loop` (2026-10-26):** all 9 Flatpak/url entries PASS.

**Bench (1080p, all 8 new apps installed, Soloist playing, 64 °C, started from Settings → bench row):** motion 60.0 fps, 0.1% slow, p95 17.3 ms, worst 29.5 ms, 1790 frames; idle 60.1 fps, 0.6% slow, p95 16.8 ms, worst 25.1 ms; pass.

**Tests:** Mac 372 + helper 57/9 + script 37; pitv 380.

**Finding:** the Home "B" bench shortcut is the keyboard letter b only (not the DualSense B/Circle or a remote key). With the remote use Settings → the panel with resolution/input-debug → third row.

## M5.5 spike report (2026-10-26, Job 1.1; moved from AGENT_JOBS.md)
Job 1.1, 2026-10-26. Nothing on pitv was changed. Facts below are command output from pitv unless a URL is given; "UNVERIFIED" = needs the proof runs in "Commands for Laith". Web-site TV usability could not be tested without launching on the TV.

**Box facts:** aarch64, kernel 6.18.50+rpt-rpi-2712, PAGESIZE 16384, Trixie, Chromium `1:155.0.8059.79-1~deb13u1+rpt1` (Raspberry Pi build; `rpi-chromium-mods` installed), flatpak 1.16.6 with user remote `flathub`, 87 GB free. No `/opt/WidevineCdm` yet. Installed flatpaks: Jellyfin Desktop, KDE Platform 6.10, Mesa GL 25.08(+extra), codecs-extra. KDE 6.11 and 5.15-25.08 runtimes are NOT installed (first install of RetroArch/Moonlight/chiaki-ng pulls KDE 6.11, VLC pulls 5.15-25.08).

### Candidate table
| Candidate | Route | aarch64 | Root | Size (dl / installed) | Fullscreen | app_id | Sign-in | Verdict |
|---|---|---|---|---|---|---|---|---|
| Spotify | open.spotify.com in kiosk Chromium + Widevine | yes (web) | yes, Widevine apt (allowlist set) | 7.5 MB deb | kiosk | `lumen-spotify` | keyboard once, profile keeps it | SHIP, pending Widevine proof (S1) |
| RetroArch | Flatpak `org.libretro.RetroArch` + cores from buildbot | yes | no | 397.5 / 749 MB (+KDE 6.11) | `retroarch --fullscreen` | UNVERIFIED (expect `org.libretro.RetroArch`) | none | SHIP (S2) |
| Moonlight | Flatpak `com.moonlight_stream.Moonlight` | yes | no | 7.1 / 15.2 MB (+KDE 6.11) | `moonlight` (fullscreen toggle in app; UNVERIFIED flag) | UNVERIFIED | PC pairing PIN once (keyboard or phone-less: needs mouse/keys) | SHIP, note: Pi 5 has no H.264 HW decode, 1080p60 software H.264 is a risk |
| chiaki-ng | Flatpak `io.github.streetpea.Chiaki4deck` (id verified) | yes | no | 232.8 / 637 MB | UNVERIFIED | UNVERIFIED | PSN login + console pairing, needs keyboard once | SHIP-conditional, heavy, Gate proof at TV |
| VLC | Flatpak `org.videolan.VLC` | yes | no | 51.6 / 157.6 MB (+KDE 5.15 runtime) | `--fullscreen` | UNVERIFIED | none | SHIP, desktop UI is weak on pad; plays files/streams via keys |
| Audiobookshelf | web `http://192.168.5.174:13378` (HTTP 200 from pitv) | yes (web) | no | 0 | kiosk | `lumen-audiobookshelf` | keyboard once | SHIP, `pause_on_home=false` useful |
| YouTube Music | web `music.youtube.com`, GoogleTV UA + youtube-tv extension idea | yes | no | 0 | kiosk | `lumen-ytmusic` | keyboard once | SHIP-conditional (UA/TV UI UNVERIFIED) |
| Twitch | web `twitch.tv` | yes | no | 0 | kiosk | `lumen-twitch` | optional | CONDITIONAL: no known TV UI, arrow nav likely poor; test at TV, drop if unusable |
| Pluto TV / Tubi | web, TV UA | yes | no | 0 | kiosk | `lumen-pluto`, `lumen-tubi` | none | CONDITIONAL, UNVERIFIED TV layout; propose both as the "1-3 more" |
| Netflix / Disney+ | web + Widevine L3 | yes | Widevine | 0 | kiosk | n/a | keyboard | DROP unless Widevine works; even then Linux Chromium caps ~720p and these sites have no arrow-key TV nav (UNVERIFIED) |
| Spotify official client | `com.spotify.Client` | NO: `flatpak remote-info --arch=aarch64 flathub com.spotify.Client` -> "Can't find ref"; flathub.org lists x86_64 only (https://flathub.org/apps/com.spotify.Client); snap/apt are amd64 | | | | | | DROP (dead end) |
| Kodi Flatpak | `tv.kodi.Kodi` exists (581 MB) | yes | | | | | | DROP (apt Kodi entry already exists, scope says so) |

Count: Spotify, RetroArch, Moonlight, VLC, Audiobookshelf are the firm five; chiaki-ng, YouTube Music, Pluto, Tubi, Twitch make 10 if the conditionals pass. Need >= 8, so expect a few TV checks before the final cut.

### S1 Spotify route
- Options: (a) official client: dead end (above). (b) open.spotify.com in Chromium + Widevine. (c) box64/Waydroid/librespot TUI: not needed unless (b) fails; box64 not in apt (`apt-cache policy box64` empty), snapd exists but Spotify snap is amd64; raspotify/librespot receivers do not meet the Gate (no lyrics app).
- Evidence for (b): Raspberry Pi repo has `libwidevinecdm0 4.10.2662.3+1` for arm64 (candidate, not installed; `apt-cache show`, Depends libc6, libnspr4; Maintainer raspberrypi.com). I downloaded the deb on the Mac: it installs to `/opt/WidevineCdm/{manifest.json,gmp-widevinecdm/latest/libwidevinecdm.so}`; the .so's LOAD segments are 64 KiB aligned, so it should map on 16K pages (page-size change probably NOT needed; only a proof run confirms). Pi build of Chromium 155 is the one Raspberry Pi pairs with this package (Tom's Hardware, https://www.tomshardware.com/news/raspberry-pi-widevine, tested on Pi 4 only; no source found for Pi 5 + 16K, so UNVERIFIED). Widevine on Linux is L3: expect <= 720p video (irrelevant for Spotify audio).
- Install: allowlist set `[widevine]`/`[spotify]` install = `["libwidevinecdm0"]`, remove = same, `root: true` (PIN pad). Or Laith by SSH.
- Connect device: the web player registers as a Connect device; name is the browser name, expected "Web Player (Chromium)" (UNVERIFIED, from memory; the proof run reads it). Needs EME so Widevine must load.
- Lyrics: no documented keyboard shortcut (Spotify shortcuts page https://support.spotify.com/na-af/article/keyboard-shortcuts/ shows none for lyrics that I could see). Needs a small MV3 extension `config/browser/spotify-tv` (like youtube-tv) that clicks the lyrics button and adds focus navigation, plus a `keys` mapping. Remappable actions are only `Up Down Left Right Select Back PlayPause`, so the lyrics toggle needs one of them (e.g. PlayPause -> a key the extension reads, with play/pause done by the extension through the media key) or a new action: Laith decides.
- Sign-in: keyboard once in the per-app profile (`~/.local/share/lumen-tv/browser/spotify`), cookies persist (same as Jellyfin/YouTube profiles).
- Kernel: none expected. If the proof fails on 16K, `kernel=kernel8.img` is a box-wide change needing a re-bench (config.txt has no kernel= line today).
- **Recommendation:** ship Spotify as web + `libwidevinecdm0` through the root helper + extension, `keep-alive`, `pause_on_home = false`; run the proof first. If Widevine does not load, write "can't ship" (the Gate allows it).

### S2 RetroArch
- Options: Flatpak `org.libretro.RetroArch` (stable, KDE 6.11, aarch64 build exists; metadata: `devices=all`, `filesystems=host;/run/udev:ro`, sockets wayland+pulseaudio+fallback-x11) vs apt `retroarch 1.20.0+dfsg-2+b1` (23 MB, Qt5/GLES deps, cores via apt `libretro-mgba 0.10.5`, `libretro-gambatte` 2016, `libretro-desmume` 2016; no melonDS in apt; apt cores are exactly the `[kodi]` remove set).
- Cores: Flatpak ships none bundled (the flathub page says to install cores with the Online Updater, https://github.com/flathub/org.libretro.RetroArch). libretro buildbot has aarch64 nightlies: `https://buildbot.libretro.com/nightly/linux/aarch64/latest/{mgba,gambatte,melonds,desmume}_libretro.so.zip` (listing fetched). Install step can fetch these with curl+unzip into `~/.var/app/org.libretro.RetroArch/config/retroarch/cores` with no root. UNVERIFIED: that the nightly .so loads inside the KDE 6.11 runtime, melonDS speed on Pi 5, and DS needs no firmware (melonDS has HLE BIOS).
- ROMs: Flatpak already has `filesystems=host`, so `~/roms` is readable with no `flatpak override`; browse via Load Content or a playlist. Saves go under `~/.var/app/org.libretro.RetroArch/` (never `~/.kodi`).
- Controller: Flatpak notes "Upstream udev support for Flatpak isn't quite there yet", falls back to the SDL joypad driver (autoconfig subset; DualSense UNVERIFIED), so a shipped `retroarch.cfg` with `input_joypad_driver = "sdl2"`/Menu binds may be needed. RetroArch reads the pad itself (like Steam Link), so Lumen's PS tap = Home may not apply while it is in front: the Gate says red = Home, which works from the remote.
- Fullscreen/lifecycle: `flatpak run org.libretro.RetroArch --fullscreen`; app_id UNVERIFIED (check with `examples/toplevels.rs` during the loop); Flatpak wrapper: set `process` if the launcher exits. `close-on-exit` + `exclusive = false`.
- `[kodi]` conflict: using Flatpak removes it: Kodi's install/remove lists stay untouched and no apt libretro package is shared.
- GPU: Pi 5 Flatpak GL uses Mesa `org.freedesktop.Platform.GL.default 26.2.4` (installed); Jellyfin Desktop had no GPU in IN-PROGRESS (2026-10-09) so check RetroArch with `glcore`/`gl` driver at the TV: UNVERIFIED.
- **Recommendation:** Flatpak + user-level core download in the install step (mGBA, Gambatte, melonDS), no root, `[kodi]` set unchanged. Fallback if cores or GPU fail: apt RetroArch with its own `[retroarch]` set that does NOT list `libretro-*` in remove (mgba/gambatte/desmume would then be shared with `[kodi]`; needs a decision first).

### S3 Final catalogue
- Ship (new): spotify (web, root: widevine), retroarch (Flatpak, no root, games), moonlight (Flatpak, games), vlc (Flatpak, media), audiobookshelf (web, media/music). Conditional, decide at the TV: chiaki-ng (games), ytmusic (web, music), twitch (web, video), pluto, tubi (web, video). Categories suggested: Media, Music, Games, Utilities.
- Drop: official Spotify, Kodi Flatpak, Netflix/Disney+ (unless Widevine proof + nav).
- Stale to remove: `kolourpaint`, `store-test`, `root-test` (+ `[root-test]`). Others: airplay description says "Uninstalling removes the tile and leaves the shared packages", fine. jellyfin and youtube remain valid, Jellyfin hard-codes `192.168.5.174` (M6). steamlink and kodi entries valid. `airplay` allowlist remove = [] so Store uninstall leaves packages: ok. Also `kodi` description mentions `~/roms` and the Games gap; add a note that RetroArch is the game player now.

### S4 Schema additions
- `arch`: needed? Only if a non-aarch64 entry ships; all candidates are aarch64 or web, so low value now but required for M6 and cheap: recommend yes.
- `category` + Store sections: yes, 10+ entries will not fit one screen.
- `notes`: yes (Moonlight pairing, chiaki PSN login, Spotify keyboard sign-in, RetroArch core download and where ROMs are).
- `pause_on_home`: yes (Spotify and Audiobookshelf must keep playing; `apps::pause_media` pauses today).
- Post-install steps: RetroArch core download fits inside `install` (multiple sh steps), no new key. Flatpak override not needed (`host` filesystem).
- Missing for Spotify: a way to bind lyrics to a key (see S1), and a Chromium launch without `--kiosk` issues? none known. Per-entry `user_agent`/`browser_args`/`keys` already exist.
- **Recommendation:** add `category`, `notes`, `arch`, `pause_on_home`; no post-install key.

### S5 Test fixtures
- Put kolourpaint/store-test/root-test in `store/fixtures/test-manifest.json`; use `--store-check store/fixtures/test-manifest.json` and `LUMEN_STORE_URL=/home/laith/lumen-tv/store/fixtures/test-manifest.json`. Root-path coverage: `helper/tests/socket.rs` and `installer.rs` already use inline `root-test` fixtures; the shipped-allowlist test just asserts the name is absent. The root PIN path can still be run manually on pitv by adding `[root-test]` to a local allowlist copy, which is a Laith/sudo step.
- Installed test apps on pitv: none. `~/.config/lumen-tv/store/` is empty, `~/.config/lumen-tv/apps/` has only airplay, jellyfin, steamlink, youtube; `dpkg -s sl` = not installed; `flatpak list --user` has no kolourpaint (leftover data only: `~/.var/app/org.kde.kolourpaint` 128 KB, harmless). So the manifest change can go live with nothing to uninstall first.
- **Recommendation:** as above; leave the 128 KB leftover alone.

### Commands for Laith (nothing run by me)
1. Widevine proof (root): `sudo apt install -y libwidevinecdm0` then `ls /opt/WidevineCdm/gmp-widevinecdm/latest/`; proves the package installs on Trixie arm64/16K. Then, with Laith's OK, launch a Chromium kiosk on the TV at `https://open.spotify.com` (own profile dir, keyboard attached) and play a track: playback without "Playback of protected content is not enabled" proves Widevine loads on the 16K kernel. Also read the Connect device name on the phone.
2. Lyrics proof: open a song with lyrics, click the mic icon; note the DOM selector (`data-testid`) via DevTools for the extension.
3. Remove after the proof (optional): `sudo apt remove -y libwidevinecdm0`.
4. RetroArch core proof (no root, writes to Pi, so Laith approves): `flatpak install --user -y --noninteractive flathub org.libretro.RetroArch`, then `curl -L -o /tmp/mgba.zip https://buildbot.libretro.com/nightly/linux/aarch64/latest/mgba_libretro.so.zip` and unzip into `~/.var/app/org.libretro.RetroArch/config/retroarch/cores/`; launch on the TV and load the GBA ROM. Proves the nightly core loads, GPU works, DualSense autoconfig.

### Read-only commands run on pitv
`uname -a; getconf PAGESIZE; grep os-release; grep config.txt (kernel/dtoverlay lines); chromium --version; apt-cache policy chromium libwidevinecdm0 retroarch flatpak vlc moonlight-qt chiaki-ng libretro-*; apt-cache show libwidevinecdm0|retroarch|chromium; apt-cache rdepends libwidevinecdm0; apt-cache search libretro; dpkg -l | grep ...; dpkg -L rpi-chromium-mods; dpkg -S /usr/lib/chromium/chromium; dpkg -s sl; df -h ~; free -m; flatpak remotes --user; flatpak list --user; flatpak remote-ls --user --arch=aarch64 flathub --columns=application; flatpak remote-info --user [--arch=aarch64] [--show-metadata] flathub <RetroArch|Moonlight|Chiaki4deck|VLC|Kodi|com.spotify.Client>; ls ~/.config/lumen-tv/{store,apps}; ls ~/.local/share/lumen-tv/browser ~/.var/app ~/roms/*; cat settings.toml env; curl -sI http://192.168.5.174:{13378,8096}/ ; cat /proc/bus/input/devices | grep Name; du -sh ~/.var/app/org.kde.kolourpaint; ls /dev/input`. On the Mac: curl of the Widevine deb and ELF header read (scratchpad only), curl of buildbot.libretro.com listings, WebSearch/WebFetch.
