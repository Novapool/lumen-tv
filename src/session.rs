// The glue between the pure lifecycle planner (apps.rs), the window control
// (platform::Windows on labwc) and the Slint model. Linux only.
//
// Everything runs on the UI thread: the Wayland dispatch thread, the process
// reapers, the poller and the input thread all hop over with
// `slint::invoke_from_event_loop`, so the `Lifecycle` needs no lock. The one
// exception is the poller's target list, mirrored into an `Arc<Mutex<..>>`.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, Weak};

use crate::apps::{self, Lifecycle, Spec, Step, WindowInfo, LUMEN_APP_ID};
use crate::platform::{toplevel, Toplevel, WindowEvent, Windows};
use crate::registry::Entry;
use crate::AppWindow;

/// How long a closing app gets to quit on its own before SIGTERM.
const CLOSE_GRACE: Duration = Duration::from_secs(2);
/// Keep asking the compositor to fullscreen Lumen's window, once a second, for this long.
const FULLSCREEN_RETRY_FOR: Duration = Duration::from_secs(10);

struct Ctx {
    lc: Lifecycle,
    entries: Vec<Entry>,
    windows: Option<Arc<dyn Windows>>,
    browser: String,
    t0: Instant,
    /// Lumen's own window is activated (the input gate reads this).
    lumen_front: Arc<AtomicBool>,
    /// `(id, process)` for the poller thread.
    targets: Arc<Mutex<Vec<(String, String)>>>,
    ui: Weak<AppWindow>,
    /// Last logged (app_id, activated, fullscreen) per window, to log changes only.
    seen: HashMap<u64, (String, bool, bool)>,
    /// Retries the fullscreen request while Lumen's window isn't fullscreen.
    fullscreen_timer: slint::Timer,
}

thread_local! {
    static CTX: RefCell<Option<Ctx>> = const { RefCell::new(None) };
}

fn with_ctx<R>(f: impl FnOnce(&mut Ctx) -> R) -> Option<R> {
    CTX.with(|c| c.borrow_mut().as_mut().map(f))
}

/// Sets up the lifecycle, connects to the compositor and wires the `launch`
/// callback. Returns the "Lumen is in front" flag for the input gate.
pub fn start(ui: &AppWindow, entries: Vec<Entry>) -> Arc<AtomicBool> {
    let lumen_front = Arc::new(AtomicBool::new(true));
    let targets = Arc::new(Mutex::new(Vec::new()));
    let lc = Lifecycle::new(entries.iter().map(Spec::from).collect());
    CTX.with(|c| {
        *c.borrow_mut() = Some(Ctx {
            lc,
            entries,
            windows: None,
            browser: apps::browser_cmd(),
            t0: Instant::now(),
            lumen_front: lumen_front.clone(),
            targets: targets.clone(),
            ui: ui.as_weak(),
            seen: HashMap::new(),
            fullscreen_timer: slint::Timer::default(),
        })
    });

    // Window events queue up on the event loop until it runs, so it is fine
    // that the windows that already exist are reported inside connect().
    match toplevel::connect(|ev| {
        let _ = slint::invoke_from_event_loop(move || window_event(ev));
    }) {
        Ok(w) => {
            with_ctx(|c| c.windows = Some(w));
        }
        Err(e) => eprintln!("[windows] no window control, apps still launch: {e}"),
    }

    apps::start_poller(
        move || targets.lock().map(|t| t.clone()).unwrap_or_default(),
        |id, pids| {
            let _ = slint::invoke_from_event_loop(move || {
                let steps = with_ctx(|c| {
                    let now = c.t0.elapsed().as_millis() as u64;
                    c.lc.process_poll(&id, &pids, now)
                });
                run(steps.unwrap_or_default());
            });
        },
    );

    let weak = ui.as_weak();
    ui.on_launch(move |i| {
        let Some(id) = weak.upgrade().and_then(|ui| ui.get_apps().row_data(i as usize)).map(|a| a.id.to_string()) else {
            return;
        };
        if id.is_empty() {
            return; // Store and Settings do nothing yet (M4, M3.5)
        }
        let steps = with_ctx(|c| {
            let now = c.t0.elapsed().as_millis() as u64;
            c.lc.select(&id, now)
        });
        run(steps.unwrap_or_default());
    });
    lumen_front
}

/// Home pressed (F1 / Super / HomePage / long Back).
pub fn home() {
    let steps = with_ctx(|c| c.lc.home(c.lumen_front.load(Ordering::Relaxed))).unwrap_or_default();
    if !steps.is_empty() {
        println!("[apps] home -> {}", describe(&steps));
    }
    run(steps);
}

fn describe(steps: &[Step]) -> String {
    steps
        .iter()
        .map(|s| match s {
            Step::Spawn(id) => format!("spawn {id}"),
            Step::Activate(w) => format!("activate window {w}"),
            Step::ActivateLumen => "activate lumen".to_string(),
            Step::Fullscreen(w) => format!("fullscreen window {w}"),
            Step::Terminate(t) => format!("terminate {}", t.id),
            Step::Pause { id, .. } => format!("pause {id}"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn windows() -> Option<Arc<dyn Windows>> {
    with_ctx(|c| c.windows.clone()).flatten()
}

fn log_err(what: &str, r: Result<(), String>) {
    if let Err(e) = r {
        eprintln!("[windows] {what}: {e}");
    }
}

/// Carries out the planner's steps, then refreshes the tiles' running dots.
fn run(steps: Vec<Step>) {
    for step in steps {
        match step {
            Step::Spawn(id) => spawn(&id),
            Step::Activate(w) => {
                println!("[apps] activate window {w}");
                if let Some(win) = windows() {
                    log_err("activate", win.activate(w));
                }
            }
            Step::ActivateLumen => {
                if let Some(win) = windows() {
                    match win.toplevels().into_iter().find(|t| t.app_id == LUMEN_APP_ID) {
                        Some(t) => log_err("activate lumen", win.activate(t.id)),
                        None => eprintln!("[windows] activate lumen: Lumen's window not found"),
                    }
                }
            }
            Step::Fullscreen(w) => {
                println!("[apps] fullscreen window {w}");
                if let Some(win) = windows() {
                    log_err("fullscreen", win.set_fullscreen(w));
                }
            }
            Step::Pause { pid, process, .. } => apps::pause_media(apps::collect_targets(pid, process.as_deref())),
            Step::Terminate(t) => {
                println!("[apps] terminate {} pid={:?} windows={:?}", t.id, t.pid, t.windows);
                // Collect before the windows close: the process tree is intact now.
                let pids = apps::collect_targets(t.pid, t.process.as_deref());
                with_ctx(|c| c.lc.retire_pids(&t.id, &pids));
                if let Some(win) = windows() {
                    for w in &t.windows {
                        log_err("close", win.close(*w));
                    }
                }
                apps::terminate(pids, CLOSE_GRACE);
            }
        }
    }
    sync_tiles();
}

fn spawn(id: &str) {
    let Some((entry, browser)) = with_ctx(|c| (c.entries.iter().find(|e| e.id == id).cloned(), c.browser.clone())) else {
        return;
    };
    let Some(entry) = entry else { return };
    let res = apps::spawn(&entry, &browser, |id, pid, code| {
        let _ = slint::invoke_from_event_loop(move || {
            println!("[apps] exited {id} pid={pid} code={code}");
            let steps = with_ctx(|c| c.lc.process_exited(&id, pid, code));
            run(steps.unwrap_or_default());
        });
    });
    match res {
        Ok(pid) => {
            // The reaper reports on the event loop (this thread), so the PID is
            // always recorded before any exit can be handled.
            with_ctx(|c| c.lc.set_pid(id, pid));
            println!("[apps] spawn {id} pid={pid}");
        }
        Err(e) => {
            eprintln!("[apps] spawn {id} failed: {e}");
            let steps = with_ctx(|c| c.lc.launch_failed(id));
            run(steps.unwrap_or_default());
        }
    }
}

/// Mirrors the lifecycle into the tiles' `running` flag and the poller's targets.
fn sync_tiles() {
    with_ctx(|c| {
        if let Ok(mut t) = c.targets.lock() {
            *t = c.lc.poll_targets();
        }
        let Some(ui) = c.ui.upgrade() else { return };
        let model = ui.get_apps();
        for i in 0..model.row_count() {
            if let Some(app) = model.row_data(i) {
                let running = !app.id.is_empty() && c.lc.is_running(&app.id);
                if running != app.running {
                    model.set_row_data(i, crate::App { running, ..app });
                }
            }
        }
    });
}

fn info(t: &Toplevel) -> WindowInfo {
    WindowInfo { id: t.id, app_id: t.app_id.clone(), activated: t.activated, fullscreen: t.fullscreen }
}

/// Timer tick: ask again until Lumen's window is fullscreen or the time is up.
fn fullscreen_retry(started: Instant) {
    let lumen = windows().and_then(|w| w.toplevels().into_iter().find(|t| t.app_id == LUMEN_APP_ID));
    let done = lumen.as_ref().is_none_or(|t| t.fullscreen) || started.elapsed() >= FULLSCREEN_RETRY_FOR;
    if done {
        with_ctx(|c| c.fullscreen_timer.stop());
    }
    if let (false, Some(t), Some(win)) = (done, lumen, windows()) {
        println!("[windows] Lumen's window is still not fullscreen, requesting it again");
        log_err("fullscreen lumen", win.set_fullscreen(t.id));
    }
}

fn window_event(ev: WindowEvent) {
    let opened = matches!(ev, WindowEvent::Opened(_));
    let steps = match ev {
        WindowEvent::Opened(t) | WindowEvent::Changed(t) => {
            let mut fullscreen_lumen = None;
            let steps = with_ctx(|c| {
                let now = (t.app_id.clone(), t.activated, t.fullscreen);
                if c.seen.insert(t.id, now.clone()) != Some(now) {
                    let what = if opened { "opened" } else { "changed" };
                    println!("[windows] {what} {:?} id={} activated={} fullscreen={}", t.app_id, t.id, t.activated, t.fullscreen);
                }
                if t.app_id == LUMEN_APP_ID {
                    c.lumen_front.store(t.activated, Ordering::Relaxed);
                    if t.fullscreen {
                        c.fullscreen_timer.stop();
                    } else if !c.fullscreen_timer.running() {
                        fullscreen_lumen = Some(t.id);
                        let started = Instant::now();
                        c.fullscreen_timer.start(slint::TimerMode::Repeated, Duration::from_secs(1), move || {
                            fullscreen_retry(started)
                        });
                    }
                }
                let now_ms = c.t0.elapsed().as_millis() as u64;
                if opened {
                    c.lc.window_opened(&info(&t), now_ms)
                } else {
                    c.lc.window_changed(&info(&t), now_ms)
                }
            });
            // The window can come up before the output is ready (tiny, top
            // left, not fullscreen). Ask for fullscreen; the size change then
            // re-bakes the tiles through `changed rem`.
            if let (Some(id), Some(win)) = (fullscreen_lumen, windows()) {
                println!("[windows] Lumen's window is not fullscreen, requesting it");
                log_err("fullscreen lumen", win.set_fullscreen(id));
            }
            steps
        }
        WindowEvent::Closed(id) => {
            println!("[windows] closed id={id}");
            with_ctx(|c| {
                c.seen.remove(&id);
                let now_ms = c.t0.elapsed().as_millis() as u64;
                c.lc.window_closed(id, now_ms)
            })
        }
    };
    run(steps.unwrap_or_default());
}
