// Lumen: an Apple TV-style launcher for the Raspberry Pi 5. M1.5: the home
// screen in Slint plus the frame-time bench. Input, registry, store and
// platform modules arrive in M2-M4.

mod bench;
mod bake;
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
            apps.set_row_data(i, App { face: bake::tile_face(&app.bg, rem), ..app });
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
