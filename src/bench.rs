// Frame-time bench (B key, or LUMEN_BENCH=1 at startup): 3 s idle, then a
// Right press every 150 ms for 30 s (bouncing at the ends), like holding an
// arrow key. Prints one `[perf] {...}` line to stdout (the journal) and shows
// the result on screen. Same maths as the M1 WebKit bench and the Slint spike,
// so the numbers compare directly.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, RenderingState, Timer, TimerMode};

use crate::AppWindow;

const IDLE: Duration = Duration::from_secs(3);
const MOTION: Duration = Duration::from_secs(30);
const STEP: Duration = Duration::from_millis(150);

#[derive(Clone, Copy, PartialEq, Default)]
enum Phase {
    #[default]
    Off,
    Idle,
    Motion,
}

/// Frame deltas from Slint's AfterRendering notifier. Slint only redraws when
/// something changes, so a gap after a frame with no running animation is the
/// UI resting, not a slow frame, and isn't counted. During the idle phase the
/// recorder forces a redraw after every frame, so idle measures how fast the
/// static scene can be redrawn.
#[derive(Default)]
struct Recorder {
    phase: Phase,
    last: Option<Instant>,
    was_animating: bool,
    deltas: Vec<f64>,
}

impl Recorder {
    fn start(&mut self, phase: Phase) {
        *self = Recorder { phase, ..Default::default() };
    }

    fn stop(&mut self) -> Option<Stats> {
        self.phase = Phase::Off;
        summarize(&std::mem::take(&mut self.deltas))
    }
}

struct Stats {
    frames: usize,
    fps: f64,
    p95: f64,
    worst: f64,
    slow_pct: f64,
}

impl Stats {
    /// The M1.5 gate (MILESTONES.md).
    fn passes(&self) -> bool {
        self.fps >= 59.0 && self.slow_pct < 2.0
    }
}

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

pub struct Bench {
    rec: RefCell<Recorder>,
    mover: Timer,
}

impl Bench {
    /// Hooks the recorder into the window's rendering notifier.
    pub fn attach(ui: &AppWindow) -> Rc<Self> {
        let bench = Rc::new(Bench { rec: RefCell::default(), mover: Timer::default() });
        let weak_ui = ui.as_weak();
        let weak_bench = Rc::downgrade(&bench);
        ui.window()
            .set_rendering_notifier(move |state, _| {
                if !matches!(state, RenderingState::AfterRendering) {
                    return;
                }
                let (Some(ui), Some(bench)) = (weak_ui.upgrade(), weak_bench.upgrade()) else { return };
                let mut r = bench.rec.borrow_mut();
                let now = Instant::now();
                match r.phase {
                    Phase::Off => return,
                    Phase::Idle => {
                        if let Some(last) = r.last {
                            r.deltas.push((now - last).as_secs_f64() * 1000.0);
                        }
                        // Request the next frame outside the rendering callback.
                        let weak_ui = weak_ui.clone();
                        Timer::single_shot(Duration::ZERO, move || {
                            if let Some(ui) = weak_ui.upgrade() {
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
        bench
    }

    pub fn run(self: &Rc<Self>, ui: &AppWindow) {
        if ui.get_input_locked() {
            return; // already running
        }
        ui.set_input_locked(true);
        ui.set_perf_verdict(0);
        ui.set_perf_text("Benchmark: idle…".into());
        ui.set_perf_detail("".into());
        self.rec.borrow_mut().start(Phase::Idle);
        ui.window().request_redraw();

        let weak_ui = ui.as_weak();
        let weak_bench = Rc::downgrade(self);
        Timer::single_shot(IDLE, move || {
            let (Some(ui), Some(bench)) = (weak_ui.upgrade(), weak_bench.upgrade()) else { return };
            let idle = bench.rec.borrow_mut().stop();
            bench.rec.borrow_mut().start(Phase::Motion);
            ui.set_perf_text("Benchmark: holding Right…".into());

            let end = Instant::now() + MOTION;
            let dir = Cell::new(1);
            let weak_bench = weak_bench.clone();
            bench.mover.start(TimerMode::Repeated, STEP, move || {
                let (Some(ui), Some(bench)) = (weak_ui.upgrade(), weak_bench.upgrade()) else { return };
                if Instant::now() < end {
                    if !ui.invoke_move(dir.get()) {
                        dir.set(-dir.get());
                        ui.invoke_move(dir.get());
                    }
                    return;
                }
                bench.mover.stop();
                let motion = bench.rec.borrow_mut().stop();
                report(&ui, &idle, &motion);
                ui.set_input_locked(false);
            });
        });
    }
}

fn report(ui: &AppWindow, idle: &Option<Stats>, motion: &Option<Stats>) {
    let size = ui.window().size();
    let pass = motion.as_ref().is_some_and(Stats::passes);
    println!(
        r#"[perf] {{"idle":{},"motion":{},"pass":{},"viewport":"{}x{}","scale":{}}}"#,
        stats_json(idle),
        stats_json(motion),
        pass,
        size.width,
        size.height,
        ui.window().scale_factor(),
    );
    match motion {
        Some(m) => {
            ui.set_perf_verdict(if pass { 1 } else { -1 });
            ui.set_perf_text(
                format!("{} · {} fps · {}% slow", if pass { "PASS" } else { "FAIL" }, m.fps, m.slow_pct).into(),
            );
            let idle_fps = idle.as_ref().map_or("–".to_string(), |s| s.fps.to_string());
            ui.set_perf_detail(
                format!("p95 {} ms · worst {} ms · idle {} fps · {}x{}", m.p95, m.worst, idle_fps, size.width, size.height)
                    .into(),
            );
        }
        None => {
            ui.set_perf_verdict(-1);
            ui.set_perf_text("Benchmark: no frames recorded".into());
        }
    }
}
