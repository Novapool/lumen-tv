// Lumen: an Apple TV-style launcher for the Raspberry Pi 5. M1.5: the home
// screen in Slint plus the frame-time bench. Input, registry, store and
// platform modules arrive in M2-M4.

mod bench;
mod platform;

use std::time::Duration;

use slint::{ComponentHandle, Timer, TimerMode};

slint::include_modules!();

fn time_now() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string()
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = AppWindow::new()?;
    ui.window().set_fullscreen(true);
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
