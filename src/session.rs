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
use crate::reclaim::Reclaim;
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
    /// Debounce for re-claiming the TV remote after an app exits.
    reclaim: Reclaim,
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
            reclaim: Reclaim::default(),
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
                update_and_run(&id, |lc, now| lc.process_poll(&id, &pids, now));
            });
        },
    );

    let weak = ui.as_weak();
    ui.on_launch(move |i| {
        let Some(id) = weak
            .upgrade()
            .and_then(|ui| ui.get_apps().row_data(i as usize))
            .map(|a| a.id.to_string())
        else {
            return;
        };
        if id.is_empty() || id.starts_with("lumen:") {
            return; // built-in tiles (Store: M4; Settings opens in the UI)
        }
        let steps = with_ctx(|c| {
            let now = c.t0.elapsed().as_millis() as u64;
            c.lc.select(&id, now)
        });
        run(steps.unwrap_or_default());
    });
    lumen_front
}

/// The registry was reloaded: replace the entries the launcher spawns from
/// and hand the specs to the planner. Apps whose entry is gone are terminated;
/// the tiles' running dots are refreshed by `run`. UI thread only.
pub fn set_entries(entries: Vec<Entry>) {
    let steps = with_ctx(|c| {
        let steps = c.lc.set_specs(entries.iter().map(Spec::from).collect());
        c.entries = entries;
        steps
    });
    if let Some(steps) = &steps {
        if !steps.is_empty() {
            println!("[apps] registry changed -> {}", describe(steps));
        }
    }
    run(steps.unwrap_or_default());
}

/// Closes a running app (before an uninstall): Lumen comes to the front and
/// the app is terminated like Home does for a close-on-exit app. Returns once
/// the steps are issued, with the PIDs that were sent the terminate (empty if
/// the app wasn't running); the exit arrives later through the reaper as usual.
pub fn close_app(id: &str) -> Vec<u32> {
    let steps = with_ctx(|c| c.lc.close(id)).unwrap_or_default();
    if !steps.is_empty() {
        println!("[apps] close {id} -> {}", describe(&steps));
    }
    run(steps)
}

/// Claims the TV remote (CEC identity + active source + keymap remap) on a
/// worker thread, since `cec-ctl` blocks. The result goes to the log and to the
/// Settings Input panel. `why` is "startup" or "re-claimed". The caller has
/// marked the claim in flight (`Reclaim::begin`/`poll`); it ends when done.
fn claim_remote_async(why: &'static str) {
    use crate::platform::Tv;
    std::thread::spawn(move || {
        let r = crate::platform::pi::PiTv::default().claim_remote();
        let (verb, line) = match &r {
            Ok(status) => (
                if why == "startup" {
                    "claimed at startup"
                } else {
                    "re-claimed"
                },
                status.clone(),
            ),
            Err(e) => (
                if why == "startup" {
                    "NOT claimed"
                } else {
                    "re-claim FAILED"
                },
                e.clone(),
            ),
        };
        let msg = format!("[input] remote {verb}: {line}");
        if r.is_ok() {
            println!("{msg}");
        } else {
            eprintln!("{msg}");
        }
        crate::diag::set_claim(r);
        let _ = slint::invoke_from_event_loop(|| {
            with_ctx(|c| c.reclaim.finished());
            try_reclaim(); // a claim that came due meanwhile
        });
    });
}

/// The claim at startup.
pub fn claim_remote_at_startup() {
    with_ctx(|c| c.reclaim.begin(c.t0.elapsed().as_millis() as u64));
    claim_remote_async("startup");
}

/// Starts a re-claim if one is due and Lumen is really in front: its window is
/// active, the planner has Lumen in the foreground and no app is still
/// launching (an exclusive launch kills the old app while Kodi starts).
fn try_reclaim() {
    let fire = with_ctx(|c| {
        let ready = c.lumen_front.load(Ordering::Relaxed)
            && c.lc.foreground() == &apps::Foreground::Lumen
            && !c.lc.snapshot().apps.iter().any(|a| a.launching);
        c.reclaim.poll(ready, c.t0.elapsed().as_millis() as u64)
    });
    if fire == Some(true) {
        claim_remote_async("re-claimed");
    }
}

/// Applies a lifecycle update for `id` and runs its steps. If that made the app
/// go away, the TV remote may need claiming again: Kodi (libcec) unregisters
/// the CEC adapter and sends inactive source on exit, so the TV leaves the
/// Pi's input.
fn update_and_run(id: &str, update: impl FnOnce(&mut Lifecycle, u64) -> Vec<Step>) {
    let was_running = with_ctx(|c| c.lc.is_running(id)).unwrap_or(false);
    let steps = with_ctx(|c| update(&mut c.lc, c.t0.elapsed().as_millis() as u64));
    run(steps.unwrap_or_default());
    if was_running && !with_ctx(|c| c.lc.is_running(id)).unwrap_or(true) {
        with_ctx(|c| c.reclaim.mark_due());
        try_reclaim();
    }
}

/// Read-only view for Settings: the lifecycle snapshot joined with the
/// compositor's windows. UI thread only. `None` before `start`.
pub fn running_view() -> Option<crate::diag::RunningView> {
    with_ctx(|c| {
        let tops = c
            .windows
            .as_ref()
            .map(|w| w.toplevels())
            .unwrap_or_default();
        crate::diag::join_running(&c.lc.snapshot(), &tops)
    })
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
/// Returns the PIDs of every `Terminate` step (for callers that wait for them).
fn run(steps: Vec<Step>) -> Vec<u32> {
    let mut terminated = Vec::new();
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
                    match win
                        .toplevels()
                        .into_iter()
                        .find(|t| t.app_id == LUMEN_APP_ID)
                    {
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
            Step::Pause { pid, process, .. } => {
                apps::pause_media(apps::collect_targets(pid, process.as_deref()))
            }
            Step::Terminate(t) => {
                println!(
                    "[apps] terminate {} pid={:?} windows={:?}",
                    t.id, t.pid, t.windows
                );
                // Collect before the windows close: the process tree is intact now.
                let pids = apps::collect_targets(t.pid, t.process.as_deref());
                with_ctx(|c| c.lc.retire_pids(&t.id, &pids));
                if let Some(win) = windows() {
                    for w in &t.windows {
                        log_err("close", win.close(*w));
                    }
                }
                terminated.extend(pids.iter().copied());
                apps::terminate(pids, CLOSE_GRACE);
            }
        }
    }
    sync_tiles();
    terminated
}

fn spawn(id: &str) {
    let Some((entry, browser)) = with_ctx(|c| {
        (
            c.entries.iter().find(|e| e.id == id).cloned(),
            c.browser.clone(),
        )
    }) else {
        return;
    };
    let Some(entry) = entry else { return };
    let res = apps::spawn(&entry, &browser, |id, pid, code| {
        let _ = slint::invoke_from_event_loop(move || {
            println!("[apps] exited {id} pid={pid} code={code}");
            update_and_run(&id, |lc, _| lc.process_exited(&id, pid, code));
            // Home or an exclusive launch has already dropped a terminated app
            // from the running set, so `update_and_run` can't see this exit.
            with_ctx(|c| c.reclaim.mark_due());
            try_reclaim();
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
        crate::input::set_foreground(match c.lc.foreground() {
            apps::Foreground::App(id) => Some(id.clone()),
            apps::Foreground::Lumen => None,
        });
        let web = match c.lc.foreground() {
            apps::Foreground::App(id) => c
                .entries
                .iter()
                .any(|e| e.id == *id && e.kind == crate::registry::Kind::Url),
            apps::Foreground::Lumen => false,
        };
        crate::input::set_foreground_web(web);
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
    WindowInfo {
        id: t.id,
        app_id: t.app_id.clone(),
        activated: t.activated,
        fullscreen: t.fullscreen,
    }
}

/// Timer tick: ask again until Lumen's window is fullscreen or the time is up.
fn fullscreen_retry(started: Instant) {
    let lumen =
        windows().and_then(|w| w.toplevels().into_iter().find(|t| t.app_id == LUMEN_APP_ID));
    let done =
        lumen.as_ref().is_none_or(|t| t.fullscreen) || started.elapsed() >= FULLSCREEN_RETRY_FOR;
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
            let lumen_active = t.app_id == LUMEN_APP_ID && t.activated;
            let steps = with_ctx(|c| {
                let now = (t.app_id.clone(), t.activated, t.fullscreen);
                if c.seen.insert(t.id, now.clone()) != Some(now) {
                    let what = if opened { "opened" } else { "changed" };
                    println!(
                        "[windows] {what} {:?} id={} activated={} fullscreen={}",
                        t.app_id, t.id, t.activated, t.fullscreen
                    );
                }
                if t.app_id == LUMEN_APP_ID {
                    c.lumen_front.store(t.activated, Ordering::Relaxed);
                    if t.fullscreen {
                        c.fullscreen_timer.stop();
                    } else if !c.fullscreen_timer.running() {
                        fullscreen_lumen = Some(t.id);
                        let started = Instant::now();
                        c.fullscreen_timer.start(
                            slint::TimerMode::Repeated,
                            Duration::from_secs(1),
                            move || fullscreen_retry(started),
                        );
                    }
                }
                let now_ms = c.t0.elapsed().as_millis() as u64;
                if opened {
                    c.lc.window_opened(&info(&t), now_ms)
                } else {
                    c.lc.window_changed(&info(&t), now_ms)
                }
            });
            if lumen_active {
                try_reclaim();
            }
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
