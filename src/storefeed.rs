// Store feed: connects store.rs (manifest, runner), home::reload and the
// lifecycle to the Store screens. Everything here runs on the UI thread;
// worker threads (fetch, install steps) hop over with `upgrade_in_event_loop`.

use std::cell::RefCell;
use std::rc::Rc;

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::store::{self, Action, JobEvent, StoreApp};
use crate::{home, registry, AppWindow, StoreItem, StoreJob};

/// Output lines kept for the progress view.
const KEEP_LINES: usize = 12;

#[derive(Default)]
struct Feed {
    /// The last good manifest (what `store-items` is built from).
    apps: Vec<StoreApp>,
    /// Newest output lines of the running job.
    lines: Vec<String>,
    /// Bumped per fetch so a stale reply is dropped.
    generation: u32,
}

thread_local! {
    static FEED: RefCell<Feed> = RefCell::new(Feed::default());
}

fn color_of(colors: Option<&[String]>) -> slint::Color {
    let hex = colors.and_then(|c| c.last()).map_or("#6b707c", |s| s.as_str());
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x6b707c);
    slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn strings(v: &[String]) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(v.iter().map(|s| SharedString::from(s.as_str())).collect::<Vec<_>>())))
}

/// What the installed tile will run: the command, or the URL for a web app.
fn launch_of(e: &registry::Entry) -> String {
    let v = if e.kind == registry::Kind::Url { &e.url } else { &e.launch };
    v.clone().unwrap_or_default()
}

/// Rebuilds `store-items` from the cached manifest, the registry and the receipts.
fn refresh_items(ui: &AppWindow) {
    let registry = registry::load(&registry::apps_dir());
    let receipts = store::read_receipts(&store::receipts_dir());
    let items: Vec<StoreItem> = FEED.with(|f| {
        f.borrow()
            .apps
            .iter()
            .map(|a| StoreItem {
                id: a.id().into(),
                name: a.entry.name.as_str().into(),
                description: a.description.as_str().into(),
                detail: a.detail.clone().unwrap_or_default().into(),
                glyph: a.entry.glyph.clone().unwrap_or_else(|| a.entry.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()).into(),
                color: color_of(a.entry.colors.as_deref()),
                installed: registry.entries.iter().any(|e| e.id == a.id()),
                needs_root: a.root,
                launch: launch_of(&a.entry).into(),
                install: strings(&a.install),
                uninstall: strings(&store::uninstall_steps(a, receipts.get(a.id()))),
            })
            .collect()
    });
    ui.set_store_items(ModelRc::from(Rc::new(VecModel::from(items))));
}

/// Fetches + parses on a worker thread, then fills the screen on the UI thread.
fn load(ui: &AppWindow) {
    let source = store::source();
    ui.set_store_state(0);
    ui.set_store_message("".into());
    ui.set_store_source(source.clone().into());
    let generation = FEED.with(|f| {
        let mut f = f.borrow_mut();
        f.generation += 1;
        f.generation
    });
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = store::load_manifest(&source);
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if FEED.with(|f| f.borrow().generation) != generation {
                return;
            }
            match result {
                Ok(m) => {
                    let message = match m.errors.first() {
                        None => String::new(),
                        Some(first) => format!("{} app(s) in the manifest were skipped: {first}", m.errors.len()),
                    };
                    FEED.with(|f| {
                        let mut f = f.borrow_mut();
                        f.apps = m.apps;
                    });
                    refresh_items(&ui);
                    ui.set_store_message(message.into());
                    ui.set_store_state(1);
                }
                Err(e) => {
                    ui.set_store_message(e.into());
                    ui.set_store_state(-1);
                }
            }
        });
    });
}

fn set_job(ui: &AppWindow, f: impl FnOnce(&mut StoreJob)) {
    let mut job = ui.get_store_job();
    f(&mut job);
    ui.set_store_job(job);
}

/// One runner event, on the UI thread.
fn on_job_event(ui: &AppWindow, ev: JobEvent) {
    match ev {
        JobEvent::Step { k, n, command } => set_job(ui, |j| {
            j.step = k as i32;
            j.steps = n as i32;
            j.command = command.into();
        }),
        JobEvent::Line(t) => push_line(ui, t, false),
        JobEvent::Replace(t) => push_line(ui, t, true),
        JobEvent::Done => {
            let uninstall = ui.get_store_job().action == "uninstall";
            reload_after_job(ui);
            let msg = if uninstall { "removed from Home" } else { "added to Home" };
            set_job(ui, |j| {
                j.state = 2;
                j.message = msg.into();
            });
        }
        JobEvent::Failed { message, .. } => {
            // A step may have changed things before it failed: Home follows the registry either way.
            reload_after_job(ui);
            set_job(ui, |j| {
                j.state = -1;
                j.message = message.into();
            });
        }
    }
}

/// Rebuilds Home from the registry, then the store list. UI thread.
fn reload_after_job(ui: &AppWindow) {
    let r = home::reload(ui);
    println!("[store] reload after job: +{:?} -{:?}", r.added, r.removed);
    refresh_items(ui);
}

/// Appends an output line (or overwrites the last one), keeping the newest 12.
fn push_line(ui: &AppWindow, text: String, replace: bool) {
    let lines = FEED.with(|f| {
        let mut f = f.borrow_mut();
        if replace && !f.lines.is_empty() {
            *f.lines.last_mut().unwrap() = text;
        } else {
            f.lines.push(text);
        }
        let extra = f.lines.len().saturating_sub(KEEP_LINES);
        f.lines.drain(..extra);
        f.lines.clone()
    });
    set_job(ui, |j| j.lines = strings(&lines));
}

fn run(ui: &AppWindow, id: &str, action: &str) {
    let action = match action {
        "install" => Action::Install,
        "uninstall" => Action::Uninstall,
        other => {
            eprintln!("[store] refused {other} {id}: unknown action");
            return;
        }
    };
    let verb = action.as_str();
    let job = ui.get_store_job();
    let fail = |ui: &AppWindow, name: &str, reason: &str| {
        eprintln!("[store] refused {verb} {id}: {reason}");
        ui.set_store_job(StoreJob {
            id: id.into(),
            name: name.into(),
            action: verb.into(),
            state: -1,
            message: reason.into(),
            ..Default::default()
        });
    };
    if job.state == 1 {
        // The one "job running" guard (the Store also opens straight to the
        // progress view while a job runs, so Run is normally unreachable).
        eprintln!("[store] refused {verb} {id}: a job is already running");
        return;
    }
    let Some(app) = FEED.with(|f| f.borrow().apps.iter().find(|a| a.id() == id).cloned()) else {
        return fail(ui, id, "not in the manifest");
    };
    let receipts = store::read_receipts(&store::receipts_dir());
    let n = match action {
        Action::Install => app.install.len(),
        Action::Uninstall => store::uninstall_steps(&app, receipts.get(id)).len(),
    };
    FEED.with(|f| f.borrow_mut().lines.clear());
    ui.set_store_job(StoreJob {
        id: id.into(),
        name: app.entry.name.as_str().into(),
        action: verb.into(),
        state: 1,
        step: 0,
        steps: n as i32,
        ..Default::default()
    });
    let name = app.entry.name.clone();
    let weak = ui.as_weak();
    // `prepare` runs after the checks pass: a running app is closed first so
    // the uninstall doesn't pull it out from under itself, and the worker
    // waits for its processes to exit before step 1.
    let started = store::run_job(
        action,
        app,
        || {
            #[cfg(target_os = "linux")]
            if action == Action::Uninstall {
                return crate::session::close_app(id);
            }
            Vec::new()
        },
        move |ev| {
            let _ = weak.upgrade_in_event_loop(move |ui| on_job_event(&ui, ev));
        },
    );
    if let Err(reason) = started {
        // The refusal is the job's Failed message (nothing was started).
        fail(ui, &name, &reason);
    }
}

/// Connects the Store callbacks. Call once, after Settings is wired.
pub fn wire(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_store_changed(move |open| {
        if let (true, Some(ui)) = (open, weak.upgrade()) {
            load(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_reload(move || {
        if let Some(ui) = weak.upgrade() {
            load(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_run(move |id, action| {
        if let Some(ui) = weak.upgrade() {
            run(&ui, &id, &action);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_dismiss(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_store_job(StoreJob::default());
        }
    });
}
