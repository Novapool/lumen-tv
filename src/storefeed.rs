// Store feed: connects store.rs (manifest, runner), home::reload and the
// lifecycle to the Store screens. Everything here runs on the UI thread;
// worker threads (fetch, install steps) hop over with `upgrade_in_event_loop`.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

use crate::platform::RootStatus;
use crate::store::{self, Action, JobEvent, StoreApp};
use crate::{home, registry, AppWindow, StoreItem, StoreJob};

/// Output lines kept for the progress view.
const KEEP_LINES: usize = 12;
/// PIN length the helper accepts.
const PIN_MIN: usize = 4;
const PIN_MAX: usize = 8;

#[derive(Default)]
struct Feed {
    /// The last good manifest (what `store-items` is built from).
    apps: Vec<StoreApp>,
    /// Newest output lines of the running job.
    lines: Vec<String>,
    /// Bumped per fetch so a stale reply is dropped.
    generation: u32,
    /// The helper's status, fetched with the manifest and after every root job.
    status: RootStatus,
    /// When the helper's current PIN lockout ends (from the status or a rejection).
    lock_until: Option<Instant>,
    /// The digits typed on the pad. Never leaves Rust: Slint only gets the count.
    pin: String,
    /// What the pad is for: (app id, action), set when the pad opens.
    pin_target: Option<(String, String)>,
    /// The running (or last) job is a root job.
    job_root: bool,
}

thread_local! {
    static FEED: RefCell<Feed> = RefCell::new(Feed::default());
}

fn color_of(colors: Option<&[String]>) -> slint::Color {
    let hex = colors
        .and_then(|c| c.last())
        .map_or("#6b707c", |s| s.as_str());
    let v = u32::from_str_radix(hex.trim_start_matches('#'), 16).unwrap_or(0x6b707c);
    slint::Color::from_rgb_u8((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn strings(v: &[String]) -> ModelRc<SharedString> {
    ModelRc::from(Rc::new(VecModel::from(
        v.iter()
            .map(|s| SharedString::from(s.as_str()))
            .collect::<Vec<_>>(),
    )))
}

/// How a Store entry reaches root: 0 not root; 1 the helper is ready (PIN);
/// 2 no helper, or the id isn't on its allowlist (SSH text); 3 the helper is up
/// but has no PIN yet.
fn root_mode(root: bool, id: &str, status: &RootStatus) -> i32 {
    if !root {
        0
    } else if !status.available || !status.sets.contains_key(id) {
        2
    } else if !status.pin_set {
        3
    } else {
        1
    }
}

/// Applies a pad key to the digit buffer: 0-9 appends (up to 8), -1 deletes.
fn apply_key(pin: &mut String, key: i32) {
    match key {
        0..=9 if pin.len() < PIN_MAX => pin.push(char::from(b'0' + key as u8)),
        -1 => {
            pin.pop();
        }
        _ => {}
    }
}

fn rejected_message(tries_left: Option<u32>) -> String {
    match tries_left {
        Some(1) => "Wrong PIN (1 try left)".into(),
        Some(n) => format!("Wrong PIN ({n} tries left)"),
        None => "Wrong PIN".into(),
    }
}

/// Whole seconds left until `until` (rounded up), 0 when it has passed.
fn lock_left(until: Option<Instant>) -> i32 {
    until.map_or(0, |t| {
        let left = t.saturating_duration_since(Instant::now());
        left.as_millis().div_ceil(1000) as i32
    })
}

fn lock_deadline(locked_for_s: u32) -> Option<Instant> {
    (locked_for_s > 0).then(|| Instant::now() + Duration::from_secs(locked_for_s as u64))
}

/// What the installed tile will run: the command, or the URL for a web app.
fn launch_of(e: &registry::Entry) -> String {
    let v = if e.kind == registry::Kind::Url {
        &e.url
    } else {
        &e.launch
    };
    v.clone().unwrap_or_default()
}

/// Rebuilds `store-items` from the cached manifest, the registry and the receipts.
fn refresh_items(ui: &AppWindow) {
    let registry = registry::load(&registry::apps_dir());
    let receipts_dir = store::receipts_dir();
    let receipts = store::read_receipts(&receipts_dir);
    let registry_ids: std::collections::HashSet<String> = registry.entries.iter().map(|e| e.id.clone()).collect();
    let items: Vec<StoreItem> = FEED.with(|f| {
        let f = f.borrow();
        // `apps` arrives grouped by category (store::store_view).
        let layout = store::section_layout(&f.apps);
        f.apps
            .iter()
            .zip(layout)
            .map(|(a, (section, section_start))| {
                let installed = store::is_installed(a, &registry_ids, &receipts_dir);
                let mode = root_mode(a.root, a.id(), &f.status);
                // Package names come only from the helper's status, never the manifest.
                let root_command = match (mode, f.status.sets.get(a.id())) {
                    (1, Some(set)) => store::root_command(
                        if installed {
                            Action::Uninstall
                        } else {
                            Action::Install
                        },
                        a.id(),
                        set,
                    ),
                    _ => String::new(),
                };
                StoreItem {
                    id: a.id().into(),
                    name: a.entry.name.as_str().into(),
                    description: a.description.as_str().into(),
                    detail: a.detail.clone().unwrap_or_default().into(),
                    glyph: a
                        .entry
                        .glyph
                        .clone()
                        .unwrap_or_else(|| {
                            a.entry
                                .name
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase().to_string())
                                .unwrap_or_default()
                        })
                        .into(),
                    color: color_of(a.entry.colors.as_deref()),
                    installed,
                    root_mode: mode,
                    root_note: if a.root {
                        f.status.note.clone().unwrap_or_default().into()
                    } else {
                        "".into()
                    },
                    root_command: root_command.into(),
                    launch: launch_of(&a.entry).into(),
                    install: strings(&a.install),
                    uninstall: strings(&store::uninstall_steps(a, receipts.get(a.id()))),
                    category: a.category.as_str().into(),
                    notes: a.notes.as_str().into(),
                    section: section as i32,
                    section_start,
                }
            })
            .collect()
    });
    ui.set_store_items(ModelRc::from(Rc::new(VecModel::from(items))));
}

/// Stores the helper's status and the lockout deadline it implies.
fn set_status(f: &mut Feed, status: RootStatus) {
    f.lock_until = lock_deadline(status.locked_for_s);
    f.status = status;
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
        // Same worker: the helper's status is asked for after the manifest (it blocks on the socket).
        let status = if result.is_ok() {
            store::root_status()
        } else {
            RootStatus::default()
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if FEED.with(|f| f.borrow().generation) != generation {
                return;
            }
            match result {
                Ok(m) => {
                    let message = match m.errors.first() {
                        None => String::new(),
                        Some(first) => format!(
                            "{} app(s) in the manifest were skipped: {first}",
                            m.errors.len()
                        ),
                    };
                    FEED.with(|f| {
                        let mut f = f.borrow_mut();
                        f.apps = store::store_view(&m.apps, &store::running_arch());
                        set_status(&mut f, status);
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

/// Asks the helper again on a worker (locks, PIN set, installed state), then
/// rebuilds the list on the UI thread.
fn refresh_status(ui: &AppWindow) {
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let status = store::root_status();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            FEED.with(|f| {
                let mut f = f.borrow_mut();
                set_status(&mut f, status);
            });
            refresh_items(&ui);
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
    let root = FEED.with(|f| f.borrow().job_root);
    match ev {
        // A root job's Step only comes once the helper has accepted the PIN.
        JobEvent::Step { k, n, command } => {
            ui.set_pin_busy(false);
            set_job(ui, |j| {
                j.step = k as i32;
                j.steps = n as i32;
                j.command = command.into();
            });
        }
        JobEvent::Line(t) => {
            push_line(ui, t, false);
        }
        JobEvent::Replace(t) => {
            push_line(ui, t, true);
        }
        JobEvent::Done => {
            ui.set_pin_busy(false);
            let job = ui.get_store_job();
            let uninstall = job.action == "uninstall";
            let tile = FEED.with(|f| f.borrow().apps.iter().find(|a| a.id() == job.id.as_str()).is_none_or(|a| a.tile));
            reload_after_job(ui);
            if root {
                refresh_status(ui);
            }
            let msg = match (tile, uninstall) {
                (true, true) => "removed from Home",
                (true, false) => "added to Home",
                (false, true) => "uninstalled",
                (false, false) => "installed",
            };
            set_job(ui, |j| {
                j.state = 2;
                j.message = msg.into();
            });
        }
        JobEvent::Failed { message, .. } => {
            ui.set_pin_busy(false);
            // A step may have changed things before it failed: Home follows the registry either way.
            reload_after_job(ui);
            if root {
                refresh_status(ui);
            }
            set_job(ui, |j| {
                j.state = -1;
                j.message = message.into();
            });
        }
        JobEvent::PinRejected {
            tries_left,
            locked_for_s,
        } => {
            // Back on the pad, digits cleared; state 0 so another try is allowed.
            FEED.with(|f| {
                let mut f = f.borrow_mut();
                f.pin.clear();
                f.lock_until = lock_deadline(locked_for_s);
            });
            ui.set_store_job(StoreJob::default());
            ui.set_pin_busy(false);
            ui.set_pin_digits(0);
            ui.set_pin_lock(locked_for_s as i32);
            let message = if locked_for_s > 0 {
                String::new()
            } else {
                rejected_message(tries_left)
            };
            ui.set_pin_message(message.into());
            refresh_status(ui);
        }
        JobEvent::NoPin => {
            FEED.with(|f| {
                let mut f = f.borrow_mut();
                f.pin.clear();
                f.status.pin_set = false;
            });
            ui.set_store_job(StoreJob::default());
            ui.set_pin_busy(false);
            ui.set_pin_digits(0);
            refresh_items(ui);
            ui.set_pin_no_pin(true);
            refresh_status(ui);
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

fn run(ui: &AppWindow, id: &str, action: &str, pin: Option<String>) {
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
        ui.set_pin_busy(false);
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
    // A root job is exactly one helper step; its package list is the helper's, not the manifest's.
    let n = match action {
        _ if app.root => 1,
        Action::Install => app.install.len(),
        Action::Uninstall => store::uninstall_steps(&app, receipts.get(id)).len(),
    };
    let status = FEED.with(|f| {
        let mut f = f.borrow_mut();
        f.lines.clear();
        f.job_root = app.root;
        f.status.clone()
    });
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
        &status,
        pin,
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

/// Drops the typed digits and the pad's target, and blanks the pad.
fn pin_clear(ui: &AppWindow) {
    FEED.with(|f| {
        let mut f = f.borrow_mut();
        f.pin.clear();
        f.pin_target = None;
    });
    ui.set_pin_digits(0);
    ui.set_pin_message("".into());
}

/// The pad opened for (id, action): fresh digits, and the lockout the helper last reported.
fn pin_open(ui: &AppWindow, id: &str, action: &str) {
    let left = FEED.with(|f| {
        let mut f = f.borrow_mut();
        f.pin.clear();
        f.pin_target = Some((id.to_string(), action.to_string()));
        lock_left(f.lock_until)
    });
    ui.set_pin_digits(0);
    ui.set_pin_message("".into());
    ui.set_pin_busy(false);
    ui.set_pin_lock(left);
}

fn pin_key(ui: &AppWindow, key: i32) {
    if ui.get_pin_busy() || ui.get_pin_lock() > 0 {
        return;
    }
    let digits = FEED.with(|f| {
        let mut f = f.borrow_mut();
        apply_key(&mut f.pin, key);
        f.pin.len()
    });
    ui.set_pin_digits(digits as i32);
    ui.set_pin_message("".into());
}

/// Sends the digits to the helper through the job runner; they are moved out of
/// the buffer here and never stored anywhere else.
fn pin_submit(ui: &AppWindow) {
    if ui.get_pin_busy()
        || ui.get_store_job().state == 1
        || FEED.with(|f| lock_left(f.borrow().lock_until)) > 0
    {
        return;
    }
    let taken = FEED.with(|f| {
        let mut f = f.borrow_mut();
        let target = f.pin_target.clone()?;
        if !(PIN_MIN..=PIN_MAX).contains(&f.pin.len()) {
            return Some((target, None));
        }
        Some((target, Some(std::mem::take(&mut f.pin))))
    });
    let Some(((id, action), pin)) = taken else {
        return;
    };
    ui.set_pin_digits(0);
    let Some(pin) = pin else {
        ui.set_pin_message(format!("PIN is {PIN_MIN}\u{2013}{PIN_MAX} digits").into());
        return;
    };
    ui.set_pin_message("".into());
    run(ui, &id, &action, Some(pin));
    // Busy until the first output, an end, or a rejection (run() fails the job itself if it can't start).
    if ui.get_store_job().state == 1 {
        ui.set_pin_busy(true);
    }
}

/// Connects the Store callbacks. Call once, after Settings is wired.
pub fn wire(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_store_changed(move |open| {
        let Some(ui) = weak.upgrade() else { return };
        if open {
            load(&ui);
        } else {
            // The Store closed: nothing typed on the pad survives.
            pin_clear(&ui);
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
            run(&ui, &id, &action, None);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_pin_open(move |id, action| {
        if let Some(ui) = weak.upgrade() {
            pin_open(&ui, &id, &action);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_pin_key(move |key| {
        if let Some(ui) = weak.upgrade() {
            pin_key(&ui, key);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_pin_submit(move || {
        if let Some(ui) = weak.upgrade() {
            pin_submit(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_pin_cancel(move || {
        if let Some(ui) = weak.upgrade() {
            pin_clear(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_store_dismiss(move || {
        if let Some(ui) = weak.upgrade() {
            ui.set_store_job(StoreJob::default());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform::RootSet;

    fn status(available: bool, pin_set: bool, ids: &[&str]) -> RootStatus {
        RootStatus {
            available,
            pin_set,
            sets: ids
                .iter()
                .map(|i| (i.to_string(), RootSet::default()))
                .collect(),
            ..Default::default()
        }
    }

    #[test]
    fn root_mode_covers_the_four_modes() {
        // 0: not a root app, whatever the helper says.
        assert_eq!(root_mode(false, "x", &status(true, true, &["x"])), 0);
        // 1: helper up, PIN set, id allowlisted.
        assert_eq!(root_mode(true, "x", &status(true, true, &["x"])), 1);
        // 2: no helper, or the id isn't allowlisted.
        assert_eq!(root_mode(true, "x", &RootStatus::default()), 2);
        assert_eq!(root_mode(true, "x", &status(true, true, &["y"])), 2);
        assert_eq!(root_mode(true, "x", &status(true, false, &["y"])), 2);
        // 3: helper up and allowlists the id, but no PIN yet.
        assert_eq!(root_mode(true, "x", &status(true, false, &["x"])), 3);
    }

    #[test]
    fn keys_append_up_to_eight_and_delete() {
        let mut pin = String::new();
        for k in [1, 2, 3, 4, 5, 6, 7, 8, 9, 0] {
            apply_key(&mut pin, k);
        }
        assert_eq!(pin, "12345678");
        apply_key(&mut pin, -1);
        apply_key(&mut pin, -1);
        assert_eq!(pin, "123456");
        apply_key(&mut pin, 0);
        assert_eq!(pin, "1234560");
        apply_key(&mut pin, 10); // not a key
        assert_eq!(pin, "1234560");
        let mut empty = String::new();
        apply_key(&mut empty, -1);
        assert_eq!(empty, "");
    }

    #[test]
    fn rejection_messages() {
        assert_eq!(rejected_message(Some(3)), "Wrong PIN (3 tries left)");
        assert_eq!(rejected_message(Some(1)), "Wrong PIN (1 try left)");
        assert_eq!(rejected_message(None), "Wrong PIN");
    }

    #[test]
    fn lock_countdown_rounds_up_and_expires() {
        assert_eq!(lock_left(None), 0);
        assert_eq!(lock_left(lock_deadline(0)), 0);
        let n = lock_left(lock_deadline(60));
        assert!((59..=60).contains(&n), "{n}");
        assert_eq!(lock_left(Some(Instant::now() - Duration::from_secs(1))), 0);
    }

    #[test]
    fn keys_outside_the_pad_are_ignored_and_the_buffer_never_exceeds_the_max() {
        let mut pin = String::from("12");
        for k in [-2, -100, 10, 11, 99, i32::MIN, i32::MAX] {
            apply_key(&mut pin, k);
        }
        assert_eq!(pin, "12");
        for _ in 0..50 {
            apply_key(&mut pin, 7);
        }
        assert_eq!(pin.len(), PIN_MAX);
        assert!(pin.bytes().all(|b| b.is_ascii_digit()));
        for _ in 0..50 {
            apply_key(&mut pin, -1);
        }
        assert_eq!(pin, "");
    }

    #[test]
    fn root_mode_ignores_the_lock_and_the_note() {
        // A locked helper is still mode 1: the pad shows the countdown on submit.
        let locked = RootStatus {
            locked_for_s: 600,
            note: Some("restart Lumen".into()),
            ..status(true, true, &["x"])
        };
        assert_eq!(root_mode(true, "x", &locked), 1);
        // Unavailable beats everything, even a PIN and the id being listed.
        let down = RootStatus {
            available: false,
            ..status(true, true, &["x"])
        };
        assert_eq!(root_mode(true, "x", &down), 2);
    }

    #[test]
    fn lock_countdown_never_shows_zero_while_time_remains() {
        let almost_over = Some(Instant::now() + Duration::from_millis(5));
        assert_eq!(lock_left(almost_over), 1);
        assert_eq!(lock_left(lock_deadline(1)), 1);
        assert!(lock_deadline(0).is_none());
        assert!(
            lock_deadline(u32::MAX).is_some(),
            "a huge lock must not overflow"
        );
    }
}
