# Lumen — original brief (2026-10-06)

Verbatim spec from Laith. Source of truth for scope. Don't edit; record scope changes in MILESTONES.md.

---

You are building Lumen, a lightweight Apple TV-style launcher for a Raspberry Pi 5. Use the existing context in this directory for anything about my current Pi setup. Before writing code, read this whole brief, then give me a short implementation plan (milestones, files you'll create, open questions). Wait for my go-ahead, then build milestone by milestone, and stop after each milestone so I can test on the TV.

Repo and package name: lumen-tv. Display name in the UI: Lumen.

## 1. Goal

A couch-first front end the Pi boots straight into. It should look polished (big glossy tiles, smooth animations), work out of the box with a TV remote and game controllers, and make adding apps trivial. It replaces Kodi entirely.

Priorities, in order:

1. Feels smooth and responsive on Pi 5 hardware
2. Input "just works" (remote, controller, keyboard)
3. Adding/installing apps is dead simple
4. Code stays small and readable

## 2. Platform and stack

- Hardware: Raspberry Pi 5, Raspberry Pi OS (64-bit), HDMI to a TV with CEC.
- Session: Auto-login, then start the labwc compositor with Lumen as the only thing on screen. No desktop, no panel.
- App shell: Tauri 2 (Rust backend) + Svelte frontend (SvelteKit not needed; plain Svelte + Vite is fine).
- Portability: Pi-only for v1, but keep all hardware-specific code (CEC, HDMI, window control, install commands) behind small Rust traits/modules so an x86 mini PC port later means swapping modules, not rewriting.

Note: Tauri on Linux renders with WebKitGTK. Measure animation performance on the Pi early (Milestone 1). If it can't hold smooth frame rates, tell me before going further and propose options.

## 3. Architecture overview

```
┌─────────────────────────────────────────────┐
│ Svelte UI (home grid, store, settings)      │
│   listens ONLY to normalized nav events     │
└──────────────▲──────────────────────────────┘
               │ Tauri events / commands
┌──────────────┴──────────────────────────────┐
│ Rust backend                                │
│  ├─ input:    evdev reader + mappings       │
│  ├─ registry: local app entries (TOML)      │
│  ├─ store:    fetch Git manifest, install   │
│  ├─ apps:     launch, track, close, focus   │
│  └─ platform: Pi-specific bits (traits)     │
└─────────────────────────────────────────────┘
```

## 4. Input layer (the heart of the project)

### Normalized events

The UI only ever receives these:

| Event | Purpose |
|---|---|
| Up Down Left Right | Move focus |
| Select | Open focused item |
| Back | Go up one level |
| Home | Return to Lumen from anywhere, including while another app is focused |
| Menu | Context options for a tile (move, uninstall, info) |
| PlayPause | Optional pass-through for media |

### Rules

- The Rust backend reads input directly from evdev (/dev/input/event*), not the webview. This is what lets Home work while Jellyfin or Steam Link has focus. Handle device hotplug (controllers connecting/disconnecting).
- Sources to support: HDMI-CEC remote (exposed by the kernel as an input device on Pi 5), gamepads (Xbox, PlayStation, Steam controllers, generic), keyboard.
- Do not exclusively grab devices while another app is focused. Other apps need raw input. When another app is in front, Lumen only acts on Home.
- Per-device mapping files (e.g. config/input/xbox.toml, config/input/cec-remote.toml, config/input/keyboard.toml) map raw codes to normalized events. Supporting a new controller should mean adding a file, not code.
- Hold-to-repeat on directions with slight acceleration.
- Analog stick deadzone (configurable) so stick drift never scrolls on its own. Sticks map to directional events.
- Home mappings: remote's Home button, controller guide button, keyboard (e.g. Super or F1). Fallback: long-press Back.

### Focus navigation

Spatial navigation: pressing a direction moves focus to the nearest focusable element in that direction. Focus never gets lost; there is always exactly one focused element.

## 5. App registry

Local, file-based. One TOML file per app in ~/.config/lumen-tv/apps/ (or similar; justify your choice).

```toml
id = "jellyfin"
name = "Jellyfin"
icon = "icons/jellyfin.png"
type = "command"            # "command" | "url"
launch = "flatpak run com.github.iwalton3.jellyfin-media-player"  # placeholder, verify
fallback_url = "http://<server>:8096"   # optional, opens in fullscreen kiosk window
background = "keep-alive"   # see lifecycle below
exclusive = false
```

### App lifecycle policies

- keep-alive: pressing Home sends Lumen to the front; the app keeps running so returning is instant. Selecting its tile again brings it back rather than relaunching.
- close-on-exit: pressing Home closes the app.
- exclusive = true: launching this app first closes all other running background apps to free RAM.

Defaults to ship:

- Jellyfin: keep-alive
- Plex: keep-alive
- Steam Link: close-on-exit + exclusive = true

If an app exits on its own, Lumen comes back to the front automatically.

### Window control

Under Wayland, Lumen cannot freely raise/hide other windows. Use what labwc supports (e.g. foreign-toplevel management or labwc actions) and keep it behind the platform trait.

## 6. Store (v1)

- A Git repo holds manifest.json: a list of available apps with the same fields as a registry entry, plus description, install (shell steps, e.g. an apt or flatpak command), and uninstall.
- Lumen fetches the manifest, shows a browsable store screen, and on install: runs the install steps (with clear progress and error states on screen), then writes the registry entry locally.
- Uninstall reverses it.
- Make the manifest URL a config value. Seed a sample manifest in this repo (store/manifest.json) with Jellyfin, Plex, and Steam Link.
- Keep install commands visible/inspectable; don't run anything from the manifest silently.

## 7. Look and feel

Apple TV style:

- Large glossy app tiles on a dark background in a horizontal row/grid.
- Focused tile scales up with a soft drop shadow, a subtle parallax tilt, and a light sheen sweep.
- Smooth transitions everywhere (focus moves, screen changes, app return). Target 60fps on the Pi; prefer GPU-friendly CSS (transform, opacity), avoid layout-thrashing animations.
- Readable at TV distance: big type, high contrast, generous spacing, safe margins for overscan.
- Screens for v1: Home grid, Store, App details/context menu (from Menu), basic Settings (input mappings view, manifest URL, about).

## 8. v1 scope

In:
- Boot-to-Lumen session setup on Pi OS (script + instructions)
- Input layer as described
- App registry + lifecycle policies
- Store pulling from Git manifest
- Home, Store, context menu, Settings screens
- Jellyfin, Plex, Steam Link entries

Out (v2+):
- Screen share / AirPlay receiver (planned: UxPlay as an app entry)
- x86 platform module
- Accounts, hosted backend, auto-updates

## 9. Things to verify on the real hardware

Do these early and report back:

- CEC remote: use cec-ctl (and check input devices) to confirm which buttons my TV remote actually sends, especially Home. Some TVs keep Home for their own menu. Generate cec-remote.toml from what's observed.
- Jellyfin client: check what currently installs cleanly on Pi 5 (Flatpak, native package, or the newer desktop client). If nothing works well, fall back to the Jellyfin web client in a fullscreen kiosk window.
- Plex: same check; the web client in kiosk mode is an acceptable fallback.
- Steam Link: confirm the current install method on 64-bit Pi OS and that it launches and exits cleanly from Lumen.
- Permissions: reading evdev needs the user in the input group (or a udev rule). Set this up in the install script.
- Performance: Tauri/WebKitGTK animation smoothness on the Pi.

## 10. Suggested milestones

1. Skeleton + perf check: Tauri + Svelte app fullscreen under labwc, static tile grid with focus animations driven by keyboard. Measure smoothness.
2. Input layer: evdev reader, mapping files, normalized events to UI, hotplug, repeat, deadzone, CEC verification.
3. Registry + launching: TOML entries, launch/return/close, lifecycle policies, Home while another app is focused.
4. Store: manifest fetch, browse, install/uninstall with progress.
5. Boot session + polish: auto-login into Lumen, Settings screen, visual polish pass, README with setup steps.

## 11. Done means

- Pi powers on and lands on Lumen with no desktop visible.
- TV remote and an Xbox controller can both navigate everything.
- Home returns to Lumen from inside Jellyfin and Steam Link.
- Jellyfin stays running in the background; Steam Link closes others on launch and closes itself on Home.
- An app can be installed from the store and appears on the home grid without restarting.
- Animations feel smooth on the TV.
- README explains setup, adding an app manually, adding a controller mapping, and pointing at a different manifest.
