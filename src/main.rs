// Lumen: an Apple TV-style launcher for the Raspberry Pi 5. The home screen
// in Slint, the app registry + lifecycle (launch, track, return), the evdev
// input layer, and the frame-time bench. The store arrives in M4.

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod apps;
mod bench;
mod bake;
mod registry;
mod icons;
// Input and platform code runs on the Pi (Linux) only; elsewhere, e.g. UI
// work on the Mac, the window's own key events drive navigation.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod input;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod platform;
#[cfg(target_os = "linux")]
mod session;

use std::rc::Rc;
use std::time::Duration;

use slint::{ComponentHandle, Model, Timer, TimerMode, VecModel};

slint::include_modules!();

fn time_now() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string()
}

/// Bakes every app's hero backdrop from its tile gradient (see bake.rs).
fn bake_heroes(ui: &AppWindow, home: &Home) {
    let t = std::time::Instant::now();
    let apps: Vec<App> =
        ui.get_apps().iter().enumerate().map(|(i, app)| App { hero: bake::hero(&home.tints[i]), ..app }).collect();
    ui.set_apps(Rc::new(VecModel::from(apps)).into());
    println!("[bake] heroes in {} ms", t.elapsed().as_millis());
}

/// What the home screen keeps beyond the Slint model, by row i of the model:
/// each tile's gradient (bake input) and icon outcome (`None` for the built-in
/// tiles), plus the registry's load errors. Nothing displays the last two
/// until M3.5's Settings.
struct Home {
    tints: Vec<bake::Tint>,
    #[allow(dead_code)] // M3.5 Settings
    errors: Vec<registry::LoadError>,
    #[allow(dead_code)] // M3.5 Settings
    icons: Vec<Option<icons::IconStatus>>,
}

/// `#rrggbb` (validated by the registry) to rgb.
fn hex_rgb(s: &str) -> [u8; 3] {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// The 135 degree tile gradient: the stops spread evenly.
fn tile_tint(colors: Option<&[String]>) -> bake::Tint {
    let default = ["#24262c".to_string(), "#6b707c".to_string()];
    let rgb: Vec<[u8; 3]> = colors.unwrap_or(&default).iter().map(|c| hex_rgb(c)).collect();
    bake::Tint::from_stops(135., &rgb)
}

/// Loads the registry (seeding the defaults on first start), decodes the
/// icons and builds the `apps` model: registry entries, then Store and Settings.
fn build_apps() -> (Vec<registry::Entry>, Vec<App>, Home) {
    let dir = registry::apps_dir();
    match registry::seed_defaults(&dir) {
        Ok(true) => println!("[registry] wrote the default apps to {}", dir.display()),
        Ok(false) => {}
        Err(e) => eprintln!("[registry] could not seed {}: {e}", dir.display()),
    }
    let loaded = registry::load(&dir);
    let (config, roots) = (registry::config_dir(), icons::Roots::system());
    let mut apps = Vec::new();
    let mut tints = Vec::new();
    let mut statuses = Vec::new();
    for e in &loaded.entries {
        let status = e.icon.as_deref().map(|spec| icons::for_entry(&e.id, spec, &config, &roots));
        let ready = matches!(status, Some(icons::IconStatus::Ready(..)));
        let glyph = e.glyph.clone().unwrap_or_else(|| e.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
        apps.push(App {
            id: e.id.clone().into(),
            running: false,
            name: e.name.clone().into(),
            glyph: if ready { "".into() } else { glyph.into() },
            hero: Default::default(),
            face: Default::default(),
        });
        tints.push(tile_tint(e.colors.as_deref()));
        statuses.push(status);
    }
    for (name, glyph, c0, c1) in [("Store", "+", "#003b3b", "#19c3b0"), ("Settings", "⚙", "#24262c", "#6b707c")] {
        apps.push(App {
            id: "".into(),
            running: false,
            name: name.into(),
            glyph: glyph.into(),
            hero: Default::default(),
            face: Default::default(),
        });
        tints.push(tile_tint(Some(&[c0.to_string(), c1.to_string()])));
        statuses.push(None);
    }
    (loaded.entries, apps, Home { tints, errors: loaded.errors, icons: statuses })
}

/// Bakes the tile faces and the shared shadow for `rem` physical pixels per rem.
fn bake_tiles(ui: &AppWindow, rem: f32, home: &Home) {
    if rem < 1. {
        return; // window not sized yet
    }
    let t = std::time::Instant::now();
    let apps = ui.get_apps();
    for i in 0..apps.row_count() {
        if let Some(app) = apps.row_data(i) {
            let icon = match home.icons.get(i) {
                Some(Some(icons::IconStatus::Ready(icon, _))) => Some(icon),
                _ => None,
            };
            apps.set_row_data(i, App { face: bake::tile_face(&home.tints[i], rem, icon), ..app });
        }
    }
    ui.set_tile_shadow(bake::tile_shadow(rem));
    println!("[bake] tiles at {rem} px/rem in {} ms", t.elapsed().as_millis());
}

fn main() -> Result<(), slint::PlatformError> {
    // The compositor tells windows apart by app_id: this is how Lumen finds
    // its own window among the apps' (apps::LUMEN_APP_ID).
    // (set_xdg_app_id needs the platform to exist first, hence select().)
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id(apps::LUMEN_APP_ID)?;
    let ui = AppWindow::new()?;
    ui.window().set_fullscreen(true);
    let (entries, model, home) = build_apps();
    ui.set_apps(Rc::new(VecModel::from(model)).into());
    let home = Rc::new(home);
    bake_heroes(&ui, &home);
    ui.on_blend(bake::blend);
    let weak = ui.as_weak();
    ui.on_rebake_tiles(move |rem| {
        if let Some(ui) = weak.upgrade() {
            bake_tiles(&ui, rem, &home);
        }
    });
    ui.set_clock(time_now().into());

    let clock = Timer::default();
    let weak = ui.as_weak();
    clock.start(TimerMode::Repeated, Duration::from_secs(10), move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_clock(time_now().into());
        }
    });

    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });

    // Select: a short press-in on the focused tile (launch(i) does the rest).
    let weak = ui.as_weak();
    ui.on_press(move || {
        let Some(ui) = weak.upgrade() else { return };
        ui.set_pressed(true);
        let weak = weak.clone();
        Timer::single_shot(Duration::from_millis(140), move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_pressed(false);
            }
        });
    });

    #[cfg(target_os = "linux")]
    {
        let front = session::start(&ui, entries);
        start_input(&ui, front);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let weak = ui.as_weak();
        ui.on_launch(move |i| {
            let id = weak.upgrade().and_then(|ui| ui.get_apps().row_data(i as usize)).map(|a| a.id);
            println!("[apps] launch {:?} (no launching off the Pi)", id.unwrap_or_default());
        });
        let _ = entries;
    }

    let bench = bench::Bench::attach(&ui);
    let weak = ui.as_weak();
    let b = bench.clone();
    ui.on_bench(move || {
        if let Some(ui) = weak.upgrade() {
            b.run(&ui);
        }
    });

    // LUMEN_BENCH=1 (in ~/.config/lumen-tv/env, which lumen-test.service loads)
    // starts the bench 3 s after launch.
    if std::env::var("LUMEN_BENCH").is_ok_and(|v| v == "1") {
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_secs(3), move || {
            if let Some(ui) = weak.upgrade() {
                bench.run(&ui);
            }
        });
    }

    ui.run()
}

/// Starts the evdev input layer and the TV remote, and hands navigation to them.
#[cfg(target_os = "linux")]
fn start_input(ui: &AppWindow, front: std::sync::Arc<std::sync::atomic::AtomicBool>) {
    use std::sync::atomic::Ordering;

    use input::Action;
    use platform::Tv;

    std::thread::spawn(|| {
        if let Err(e) = platform::pi::PiTv::default().claim_remote() {
            eprintln!("[input] TV remote not claimed: {e}");
        }
    });

    let debug = std::env::var("LUMEN_INPUT_DEBUG").is_ok_and(|v| v == "1");
    let weak = ui.as_weak();
    input::start(move |action, device| {
        // While an app is in front only Home is Lumen's: the app gets the rest.
        let lumen_front = front.load(Ordering::Relaxed);
        if !lumen_front && action != Action::Home {
            if debug {
                println!("[input] {action:?} from \"{device}\" ignored (app in front)");
            }
            return;
        }
        if debug {
            println!("[input] {action:?} from \"{device}\"");
        }
        let label = debug.then(|| format!("{action:?} · {device}"));
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if action == Action::Home {
                session::home();
            }
            if action == Action::Home && !lumen_front {
                return; // the jump to the first tile is for Home pressed on Lumen
            }
            ui.invoke_nav(match action {
                Action::Up => NavEvent::Up,
                Action::Down => NavEvent::Down,
                Action::Left => NavEvent::Left,
                Action::Right => NavEvent::Right,
                Action::Select => NavEvent::Select,
                Action::Back => NavEvent::Back,
                Action::Home => NavEvent::Home,
                Action::Menu => NavEvent::Menu,
                Action::PlayPause => NavEvent::PlayPause,
            });
            if let Some(label) = label {
                ui.set_input_debug(label.into());
            }
        });
    });
    ui.set_key_nav(false);
}
