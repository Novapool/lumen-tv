# In Progress — Lumen (lumen-tv)

Last updated: 2026-10-26

## Where we are
Lumen's UI is Slint (femtovg), 60 fps at 1080p60 and 4K60 on the TV. Done through **M5.3 (2026-10-26)**: registry + launching (Jellyfin, YouTube, AirPlay, Steam Link), Settings/diagnostics, the LG remote (red = Home), the Store (flatpak `--user`; apt apps through the PIN-protected root helper), DualSense + Bluetooth panel, and the boot session. **Lumen is pitv's boot default** (`lumen.service` enabled, quiet boot; stop booting it = plain tty1 console login: `sudo sh ~/lumen-tv/session/install.sh --apply --revert && sudo reboot`). A Power tile restarts Lumen, reboots or shuts down. M5.1 added default YouTube (youtube.com/tv in kiosk Chromium, keep-alive) and AirPlay (UxPlay inside labwc) tiles. **Kodi is a Store app now and is uninstalled from pitv**. The crash rescue is a tty1 login prompt (3 exits in 60 s, no autologin, SSH works). Kodi-era units, scripts and sudoers files were removed from pitv (K5, 2026-10-26). Laith has no retro games until RetroArch ships in M5.5. **Next: confirm the M5.5 Gate, then the Phase 6 spike.** M5.2 (Steam Link in the background) is on hold (Laith, 2026-10-25).

## Active Plan
Order: M5.5 → M6 (M5.2 on hold) (see MILESTONES.md), stopping after each one for a TV test. M5.3 closed 2026-10-26 (build phases in docs/ARCHIVE.md); K5 removals done on pitv 2026-10-26.

**M5.2:** on hold (Laith, 2026-10-25). Spike stopped; its Pi changes undone.

Layout (as built):
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
    home.rs                Home tiles/heroes; live reload from the registry
    store.rs, storefeed.rs Store: manifest, step runner, receipts; Slint wiring
    bluetooth.rs, btfeed.rs Bluetooth panel: pure parsing/ordering/errors; Slint wiring
    input/keys_out.rs      controller → keys for web apps (uinput "lumen-keys")
    platform/bluez.rs      BlueZ via busctl + scan_slot.rs/agent_slot.rs (bluetoothctl children); fake_bt.rs = LUMEN_BT=fake
    input/                 evdev reader, hotplug, mapping loader, repeat/deadzone, dispatch
    platform/mod.rs        traits: Windows, Tv
    platform/pi.rs         CEC remote claim (cec-ctl) + OK/Back keymap remap
    platform/toplevel.rs   labwc wlr-foreign-toplevel
    platform/output.rs     HDMI mode watcher (re-applies the saved mode)
    settings.rs            ~/.config/lumen-tv/settings.toml (mode, store_url)
  ui/                      app.slint (screens, focus), settings.slint, bluetooth.slint, store.slint, details.slint, tile.slint, theme.slint
  config/input/            keyboard, xbox, playstation, gamepad, cec-remote (.toml)
  config/apps/             default registry entries; config/kodi-tile = launcher for the Kodi Store entry (CEC off)
  examples/                fake-pad.rs, press.rs, toplevels.rs (gate tools)
  store/manifest.json      the Store's app list (served from GitHub raw)
  session/                 labwc rc.xml, start-lumen.sh, lumen.service (boot unit), lumen-test.service, install.sh (dry run unless --apply), sudoers
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
- (2026-10-06, superseded 2026-10-25) Kodi stayed as a Lumen tile until replacements existed. M5.3 made it a Store app (Laith) and it is uninstalled from pitv.
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
- (2026-10-13) The Kodi launcher `config/kodi-tile` (own home `~/.kodi-tile`, userdata linked to `~/.kodi`, CEC peripheral forced off so Lumen keeps the remote) is now the Kodi Store entry's launcher, run from `~/lumen-tv/config/kodi-tile` (M5.3 K1); the old copy `~/.local/bin/kodi-tile` was removed (K5). After any app exits Lumen re-claims the remote + active source (`src/reclaim.rs`). Backup `~/.kodi-backup-20261013`.
- (2026-10-13) Plex removed (Laith): Jellyfin covers it.

- (2026-10-20) Store: manifest `store/manifest.json` (version 1; each app = registry fields + `description`, `detail`, `install`, `uninstall`, `root`), fetched from `LUMEN_STORE_URL` (default `https://raw.githubusercontent.com/Novapool/lumen-tv/main/store/manifest.json`; a `/path` or `file://` reads from disk). `lumen-tv --store-check [source]` prints it. Steps run as `sh -c` as laith, one job at a time, stop at the first failure, no rollback. Registry TOML + receipt (`~/.config/lumen-tv/store/<id>.json`, keeps the uninstall steps used) are written only after every step passes.
- (2026-10-20) Root policy (Laith): the Store runs user-level steps only (flatpak `--user`); `root: true` entries originally showed their commands to run over SSH; since M5.3 they go through the PIN-protected root helper when it is installed and fall back to the SSH text otherwise. Everything is uninstallable, Jellyfin included.
- (2026-10-20) Lumen clears ambient capabilities for every child (`apps::drop_ambient_caps`): lumen-test's PAM session gives it CAP_WAKE_ALARM, and bwrap (Flatpak) refuses to run with it.
- (2026-10-20) KolourPaint (the M4 gate app): app_id `org.kde.kolourpaint`, no `process` needed (Lumen tracks bwrap; the app is its child), icon from `~/.local/share/flatpak/exports`.

- (2026-10-20) Bluetooth (M4.5): no D-Bus crate. Snapshot = `busctl --list tree org.bluez` + per-object `get-property --json=short` (GetManagedObjects JSON fails on ManufacturerData's integer keys). Scan = a held `bluetoothctl --timeout 60 scan on` child; it stops after 60 s, when leaving the device rows, on panel/Settings close, and before pair/connect (shared Wi-Fi/BT radio). Pairing needs a default agent: while the panel is open Lumen holds a bluetoothctl session (`agent off` → `agent NoInputNoOutput` → `default-agent`); pair = Trusted → Pair → Trusted again → check Bonded → Connect. Without the agent the DualSense paired but didn't bond and BlueZ refused its HID link. Battery: BlueZ Battery1, else `/sys/class/power_supply/*<mac>*/capacity`. Device list: named only, paired → gamepads → signal, max 20, stable positions. No sudo needed (laith reaches org.bluez).
- (2026-10-20) DualSense: main node → `playstation.toml` (Cross Select, Circle Back, Options Menu, PS Home, left stick + d-pad, deadzone 0.4); Touchpad and Motion Sensors nodes are ignored by Lumen (the touchpad still moves labwc's pointer; Laith kept it on). Home rules: a PS tap is Home except inside Steam Link, where a held PS (≥ 0.8 s) is Home and a tap belongs to Steam; a controller's held Circle is plain Back while any app is in front (Home only in Lumen's own screens); keyboard F1 and remote red unchanged.
- (2026-10-20) While a `type = "url"` app is in front, controller actions become keys on Lumen's uinput keyboard "lumen-keys" (arrows, Enter, KEY_BACK, play/pause); Lumen's reader ignores that device. Steam Link and Kodi read the pad themselves.

- (2026-10-25) Per-app registry fields (M5.1): `user_agent`, `browser_args` (each `--…`, `~/` expanded to `$HOME`, shell-quoted; Chromium honours only the last `--disable-features=`), `keys` (`[keys] Back = "KEY_ESC"`: pad keys for web apps, and the CEC keymap for any app while it's in front; Home/Menu never remappable). Every remote claim rewrites the full keymap (rc-cec defaults read on pitv: arrows 103/108/105/106, PlayPause 0x44=200 / 0x46=201; Select 28, Back 158 via Lumen). New defaults reach existing installs once through `apps/.shipped`; a deleted default stays deleted.
- (2026-10-25) YouTube: `https://www.youtube.com/tv` with the GoogleTV user agent (candidates in youtube.toml), `config/browser/youtube-tv` extension (MV3, loaded with `--disable-features=DisableLoadExtensionCommandLineSwitch` on Chromium 155) hides AV1 and >1080p; plays avc1. Back = Esc.
- (2026-10-25) AirPlay: `session/lumen-airplay` (Python + GTK 3 waiting window app_id `lumen-airplay`, binds the app) runs `uxplay -n pitv -nh -fs -vs 'waylandsink fullscreen=true' -avdec -as pulsesink`; video window app_id `uxplay` stays unbound. Exits 5 s after the phone stops or 120 s with no client; PR_SET_PDEATHSIG kills uxplay if the wrapper dies. Kodi's `uxplay.service` + `airplay-mirror` were removed from pitv (K5).
- (2026-10-25) Root helper (M5.3) paths: binary `/usr/local/libexec/lumen-helper`, socket `/run/lumen-helper.sock` (root:`lumen-helper` 0660; `LUMEN_HELPER_SOCKET` on the Lumen side), allowlist `/etc/lumen-tv/helper-allowlist.toml` (source `session/helper-allowlist.toml`), state `/var/lib/lumen-helper/`, units `lumen-helper.socket` (enabled) + `.service` (idle exit 60 s). Installed by `install.sh --apply --helper`; PIN set over SSH with `lumen-helper set-pin`. Steps and removal: README "Root installs (PIN helper)".
- (2026-10-25) Helper protocol v1: one JSON request line per connection (`op` status|install|remove, `id`, `pin` 4–8 digits, `v`), streamed `line`/`replace`/`step` lines, then one final line (`status`, `done`, `failed`, `bad_pin`, `locked`, `no_pin`, `not_allowed`, `busy`, `error`). Unknown request fields are errors; the PIN is never echoed or logged. Full contract: `helper/src/proto.rs`.

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
M5.3 closed 2026-10-26 (all 12 Gate checks pass; result line in MILESTONES.md, per-check evidence in docs/ARCHIVE.md). Nothing is being built. K5 removals done on pitv; manifest pushed.

**pitv state to remember (2026-10-26):** M5.3 pushed (22bb762); the public manifest has the kodi/root-test entries and pitv's Store reads it again (the Gate's temporary `store_url` line is removed). The root helper is installed (`lumen-helper.socket` enabled, PIN set). Kodi and all its packages are gone; Kodi-era units, sudoers and scripts removed (K5).

**`[kodi]` allowlist set (K6):** install = kodi, kodi-game-libretro, kodi-inputstream-adaptive, libretro-mgba, libretro-gambatte, libretro-desmume, kodi-repository-kodi; remove = those seven + kodi-visualization-spectrum (apt simulation: 8 removed, 0 installed; the helper refuses anything apt would remove or install beyond the list). Helper: `helper/` crate (`lumen-helper serve|set-pin|check`), argon2id PIN under `/var/lib/lumen-helper/`, 5 wrong → 60 s lock doubling to 1 h. Lumen side: `src/platform/installer.rs`, `store::run_root`, `ui/pinpad.slint`. Review findings: docs/ARCHIVE.md → "M5.3 Phase 2 security review".

**Kodi inventory (Job 1.4, read-only; full tables and apt simulations in docs/ARCHIVE.md → "M5.3 Kodi inventory")**
- Saves (RetroPlayer): `~/.kodi/saves/<rom filename>/`, `~/.kodi/userdata/Savestates/game.libretro.desmume/`, `~/.kodi/userdata/addon_data/game.libretro.desmume/save/`. All EMPTY; no .sav/.srm/.dsv/.state anywhere on the Pi, so nothing to import into RetroArch. Autosave was on.
- Never touch: ~/.kodi (~245M), ~/.kodi-tile (symlinks into ~/.kodi), ~/.kodi-backup-20261013 (238M), ~/roms (1.1G: gb 1, gba 1, nds 4).
- Steam Link (`~/.local/share/SteamLink`, tile `launch = "steamlink"`) and UxPlay/GStreamer don't depend on Kodi. No getty autologin override exists (K4).

Contracts from M5 (as built): mode precedence settings.toml `mode` > `LUMEN_MODE` > 1920x1080@60; store precedence settings.toml `store_url` > `LUMEN_STORE_URL` > default. `platform/output.rs` polls wlr-randr every 2 s and re-applies the saved mode. `lumen.service`: `Restart=always`, `StartLimitBurst=3` in 60 s, then `OnFailure=getty@tty1.service` (console rescue, no autologin); `lumen-test.service` ExecStopPost starts `lumen.service`. Power: `ui/power.slint` + `src/power.rs` (Restart Lumen = exit; reboot/poweroff via `systemctl`, allowed by polkit for the active tty1 session).

## Known issues
- Lumen is the boot session: starting `lumen-test.service` stops `lumen.service`, and stopping lumen-test starts `lumen.service` again (ExecStopPost). To restart Lumen use Power → Restart Lumen, or `sudo systemctl restart lumen.service`. 3 exits in 60 s leave a tty1 login prompt (no autologin, SSH works). Bring Lumen back with `sudo systemctl reset-failed lumen.service && sudo systemctl start lumen.service`.
- `ssh pitv` uses the IP (192.168.1.60), since `pitv.local` (mDNS) doesn't resolve over the VPN.
- `/dev/uinput` is writable by the input group: `cargo run --release --example fake-pad [secs]` on the Pi tests the input layer without a controller.
- Perf lines log under `labwc[...]`, so grep the whole journal, not `-u lumen-test`.
- Don't run wf-recorder on the Pi: its frames come out garbled and it froze the TV output.
- History: the WebKit focus-change flash/snap (compositor drew pre-animation values for ~2 frames) was patched with "hold" animations (95dc6f9); gone since the Slint port.
- Benchmark: put `LUMEN_BENCH=1` in `~/.config/lumen-tv/env` (runs on every start; remove it after), or press B on the home screen.

- Home (`KEY_F1`/`KEY_LEFTMETA`) is read from evdev, so the focused app also receives it unless rc.xml swallows it (F1 and Super_L are; `KEY_HOMEPAGE` is not).
- Steam Link needs Xwayland, and labwc must be restarted after installing it (restart Lumen).
- Flatpak apps: the launched process is a wrapper, so descendants (`bwrap`/app) may outlive close; set `process` and check the terminate path before shipping one.

- DualSense reconnect: once at the M4.5 TV test, the first PS press after turning the pad off never reached the Pi (no BlueZ log); the next presses reconnected in 5-6 s. Watch it; an M5 option is asking BlueZ to connect known pads.

## Blockers
- (none)
- Bench is thermally sensitive: the Pi has no fan; after long builds it throttles at 75-82 C and 4K drops to 45-55 fps (a cool 4K run passed at 60). Let it cool before benching.

## Next Steps
1. **M5.2 on hold** (Laith, 2026-10-25). When it resumes: spike finding so far is that Steam Link ships SDL3 + HIDAPI and may read the pad via `/dev/hidraw*`; also revisit `exclusive`.
2. **K5 Pi-side:** done 2026-10-26 (Laith ran the removals of kodi/steamlink/uxplay units, their sudoers, `~/.local/bin` scripts, `kodi.toml.bak`, kodi-bin/kodi-data). Lumen units no longer name them in `Conflicts=` (repo; reinstall with `install.sh --apply` pending). Open: the Kodi-era files in `../config/` (not under git; delete only with Laith's OK).
3. Confirm the proposed M5.5 and M6 Gate lines with Laith before either starts, then run the M5.5 Phase 6 spike (RetroArch is a priority: it replaces Kodi's RetroPlayer).
4. Untested: uninstalling a keep-alive app while it runs (M4's 8 s wait).
5. Small: 5 known clippy warnings (`bluetooth.rs:432`, `icons.rs:42/70`, `input/mapping.rs:296`, `store.rs:463`). pitv has no clippy component (`rustup component add clippy`, Laith's call).

## Recently Completed
- M5.3 Root install helper (PIN) + Kodi as a Store app (2026-10-26): all 12 Gate checks pass at the TV (Laith). Kodi uninstalled from pitv; crash rescue = tty1 login. Result line in MILESTONES.md, checklist + build phases in docs/ARCHIVE.md.
- M5.1 YouTube + AirPlay tiles (2026-10-25): both Gates pass at the TV (Laith). Result line in MILESTONES.md, checklist + build-round notes in docs/ARCHIVE.md.
- M5 Boot session + polish (2026-10-24): all TV checks pass (Laith). Lumen is the boot default; Power tile; saved resolution survives TV power-cycles; stick rebound fixed; installer + quiet boot. Result line in MILESTONES.md, checklist in docs/ARCHIVE.md.
