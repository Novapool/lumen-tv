// Lumen: an Apple TV-style launcher for the Raspberry Pi 5. The home screen
// in Slint, the evdev input layer, and the frame-time bench. Registry and
// store arrive in M3-M4.

mod bench;
mod bake;
#[allow(dead_code)]
mod icons;
// Input and platform code runs on the Pi (Linux) only; elsewhere, e.g. UI
// work on the Mac, the window's own key events drive navigation.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod input;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod platform;

use std::time::Duration;

use slint::{ComponentHandle, Model, Timer, TimerMode, VecModel};

slint::include_modules!();

fn time_now() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string()
}

/// Bakes every app's hero backdrop from its tile brush (see bake.rs).
fn bake_heroes(ui: &AppWindow) {
    let t = std::time::Instant::now();
    let apps: Vec<App> = ui.get_apps().iter().map(|app| App { hero: bake::hero(&app.bg), ..app }).collect();
    ui.set_apps(std::rc::Rc::new(VecModel::from(apps)).into());
    println!("[bake] heroes in {} ms", t.elapsed().as_millis());
}

/// Bakes the tile faces and the shared shadow for `rem` physical pixels per rem.
fn bake_tiles(ui: &AppWindow, rem: f32) {
    if rem < 1. {
        return; // window not sized yet
    }
    let t = std::time::Instant::now();
    let apps = ui.get_apps();
    for i in 0..apps.row_count() {
        if let Some(app) = apps.row_data(i) {
            apps.set_row_data(i, App { face: bake::tile_face(&app.bg, rem, None), ..app });
        }
    }
    ui.set_tile_shadow(bake::tile_shadow(rem));
    println!("[bake] tiles at {rem} px/rem in {} ms", t.elapsed().as_millis());
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.window().set_fullscreen(true);
    bake_heroes(&ui);
    ui.on_blend(bake::blend);
    let weak = ui.as_weak();
    ui.on_rebake_tiles(move |rem| {
        if let Some(ui) = weak.upgrade() {
            bake_tiles(&ui, rem);
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

    // Select: a short press-in on the focused tile. Launching arrives in M3.
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
    start_input(&ui);

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
fn start_input(ui: &AppWindow) {
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
        if debug {
            println!("[input] {action:?} from \"{device}\"");
        }
        let label = debug.then(|| format!("{action:?} · {device}"));
        let _ = weak.upgrade_in_event_loop(move |ui| {
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
