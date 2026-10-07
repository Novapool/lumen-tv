// Slint spike: the M1 home screen with the same 30 s benchmark as the WebKit
// build (ui/src/lib/perf.js), so the frame times compare directly.
//
// LUMEN_BENCH=1: wait 3 s, record 3 s idle, then move Right every 150 ms for
// 30 s (bouncing at the ends) and print one `[perf] {...}` line to stdout.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, RenderingState, Timer, TimerMode};

slint::include_modules!();

#[derive(Clone, Copy, PartialEq)]
enum Phase {
    Off,
    Idle,
    Motion,
}

/// Frame deltas from Slint's AfterRendering notifier. Slint only redraws when
/// something changes, so a gap after a frame with no running animation is the
/// UI resting, not a slow frame, and isn't counted. During the idle phase the
/// recorder forces a redraw after every frame, so idle measures how fast the
/// static scene can be redrawn (WebKit's rAF also ticks every vsync at rest).
struct Recorder {
    phase: Phase,
    last: Option<Instant>,
    was_animating: bool,
    deltas: Vec<f64>,
}

impl Recorder {
    fn start(&mut self, phase: Phase) {
        self.phase = phase;
        self.last = None;
        self.was_animating = false;
        self.deltas.clear();
    }

    fn stop(&mut self) -> Vec<f64> {
        self.phase = Phase::Off;
        std::mem::take(&mut self.deltas)
    }
}

struct Stats {
    frames: usize,
    fps: f64,
    p95: f64,
    worst: f64,
    slow_pct: f64,
}

/// Same maths as summarize() in ui/src/lib/perf.js.
fn summarize(deltas: &[f64]) -> Option<Stats> {
    if deltas.is_empty() {
        return None;
    }
    let total: f64 = deltas.iter().sum();
    let mut sorted = deltas.to_vec();
    sorted.sort_by(f64::total_cmp);
    let round = |x: f64| (x * 10.0).round() / 10.0;
    let slow = deltas.iter().filter(|&&d| d > 25.0).count();
    Some(Stats {
        frames: deltas.len(),
        fps: round(1000.0 * deltas.len() as f64 / total),
        p95: round(sorted[(0.95 * (sorted.len() - 1) as f64).floor() as usize]),
        worst: round(sorted[sorted.len() - 1]),
        slow_pct: round(100.0 * slow as f64 / deltas.len() as f64),
    })
}

fn stats_json(s: &Option<Stats>) -> String {
    match s {
        None => "null".into(),
        Some(s) => format!(
            r#"{{"frames":{},"fps":{},"p95":{},"worst":{},"slowPct":{}}}"#,
            s.frames, s.fps, s.p95, s.worst, s.slow_pct
        ),
    }
}

fn time_now() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string()
}

fn main() -> Result<(), slint::PlatformError> {
    let ui = Home::new()?;
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

    let rec = Rc::new(RefCell::new(Recorder {
        phase: Phase::Off,
        last: None,
        was_animating: false,
        deltas: Vec::new(),
    }));

    let weak = ui.as_weak();
    let r = rec.clone();
    ui.window()
        .set_rendering_notifier(move |state, _| {
            if !matches!(state, RenderingState::AfterRendering) {
                return;
            }
            let Some(ui) = weak.upgrade() else { return };
            let mut r = r.borrow_mut();
            let now = Instant::now();
            match r.phase {
                Phase::Off => return,
                Phase::Idle => {
                    if let Some(last) = r.last {
                        r.deltas.push((now - last).as_secs_f64() * 1000.0);
                    }
                    // Request the next frame outside the rendering callback.
                    let weak = weak.clone();
                    Timer::single_shot(Duration::ZERO, move || {
                        if let Some(ui) = weak.upgrade() {
                            ui.window().request_redraw();
                        }
                    });
                }
                Phase::Motion => {
                    if let (Some(last), true) = (r.last, r.was_animating) {
                        r.deltas.push((now - last).as_secs_f64() * 1000.0);
                    }
                    r.was_animating = ui.window().has_active_animations();
                }
            }
            r.last = Some(now);
        })
        .expect("rendering notifier not supported by this renderer");

    if std::env::var("LUMEN_BENCH").is_ok_and(|v| v == "1") {
        let weak = ui.as_weak();
        Timer::single_shot(Duration::from_secs(3), move || run_bench(weak, rec));
    }

    ui.run()
}

fn run_bench(weak: slint::Weak<Home>, rec: Rc<RefCell<Recorder>>) {
    let Some(ui) = weak.upgrade() else { return };
    ui.set_input_locked(true);
    ui.set_perf_text("Benchmark: idle…".into());
    rec.borrow_mut().start(Phase::Idle);
    ui.window().request_redraw();

    Timer::single_shot(Duration::from_secs(3), move || {
        let Some(ui) = weak.upgrade() else { return };
        let idle = summarize(&rec.borrow_mut().stop());
        rec.borrow_mut().start(Phase::Motion);
        ui.set_perf_text("Benchmark: holding Right…".into());

        let end = Instant::now() + Duration::from_secs(30);
        let dir = RefCell::new(1);
        let mover = Rc::new(Timer::default());
        let mover_ref = Rc::downgrade(&mover);
        let weak = weak.clone();
        mover.start(TimerMode::Repeated, Duration::from_millis(150), move || {
            let Some(ui) = weak.upgrade() else { return };
            if Instant::now() < end {
                let mut d = dir.borrow_mut();
                if !ui.invoke_move(*d) {
                    *d = -*d;
                    ui.invoke_move(*d);
                }
                return;
            }
            let motion = summarize(&rec.borrow_mut().stop());
            let size = ui.window().size();
            let backend = std::env::var("SLINT_BACKEND").unwrap_or_else(|_| "default".into());
            println!(
                r#"[perf] {{"idle":{},"motion":{},"viewport":"{}x{}","scale":{},"backend":"{}"}}"#,
                stats_json(&idle),
                stats_json(&motion),
                size.width,
                size.height,
                ui.window().scale_factor(),
                backend
            );
            let summary = match &motion {
                Some(m) => format!("{} fps · {}% slow · p95 {} ms · worst {} ms", m.fps, m.slow_pct, m.p95, m.worst),
                None => "no frames recorded".into(),
            };
            ui.set_perf_text(summary.into());
            ui.set_input_locked(false);
            if let Some(t) = mover_ref.upgrade() {
                t.stop();
            }
        });
        // Keep the timer alive until it stops itself.
        std::mem::forget(mover);
    });
}
