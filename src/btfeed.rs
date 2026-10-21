// Bluetooth feed: connects the backend (platform::Bluetooth), the pure logic
// in bluetooth.rs and the Bluetooth panel in Settings. State lives on the UI
// thread; blocking calls (snapshot, scan, pair, ...) run on worker threads
// that hop back with `upgrade_in_event_loop`, like storefeed.rs.
//
// The panel tells us when it is showing (`bt-changed`): only then does the 1 s
// poll run, and leaving it (or closing Settings) stops the scan; so does
// leaving the device rows for the panel list (`bt-leave-rows`).

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, Timer, TimerMode, VecModel};

use crate::bluetooth::{self, BtRow, Note};
use crate::platform::{self, Bluetooth, BtSnapshot};
use crate::{AppWindow, BtDevice, BtState};

/// How long a result message ("Paired ...", an error) stays in the header.
const NOTE_SECS: u64 = 6;

struct Feed {
    backend: Arc<dyn Bluetooth>,
    /// The panel is showing (bt-changed(true) and not yet false).
    active: bool,
    /// A snapshot worker is in flight.
    polling: bool,
    snapshot: BtSnapshot,
    /// The last snapshot failed: friendly text.
    error: Option<String>,
    scan: bluetooth::ScanTimer,
    note: Option<(Note, Option<Instant>)>,
    /// (address, header text) of the device a pair/connect/... worker is on
    /// (one at a time). The header text comes from here, so notes can't hide it.
    busy: Option<(String, String)>,
    /// Bumped whenever an action or scan change finishes; a poll that started
    /// under an older value is stale and gets redone.
    gen: u64,
    rows: Vec<BtRow>,
    model: Rc<VecModel<BtDevice>>,
    timer: Timer,
}

thread_local! {
    static FEED: RefCell<Option<Feed>> = const { RefCell::new(None) };
}

fn with<R>(f: impl FnOnce(&mut Feed) -> R) -> Option<R> {
    FEED.with(|c| c.borrow_mut().as_mut().map(f))
}

fn slint_device(r: &BtRow) -> BtDevice {
    BtDevice {
        address: r.address.as_str().into(),
        name: r.name.as_str().into(),
        kind: r.kind.into(),
        paired: r.paired,
        connected: r.connected,
        battery: r.battery,
        busy: r.busy,
    }
}

/// Puts `rows` into `bt-devices`. Same length: only the rows that changed are
/// touched in place, so focus and scroll survive. Different length: a new model,
/// which is what the Slint row clamp (`changed bt-devices`) listens for.
fn apply_rows(ui: &AppWindow, f: &mut Feed, rows: Vec<BtRow>) {
    if rows.len() == f.rows.len() {
        for (i, r) in rows.iter().enumerate() {
            if f.rows[i] != *r {
                f.model.set_row_data(i, slint_device(r));
            }
        }
    } else {
        f.model = Rc::new(VecModel::from(rows.iter().map(slint_device).collect::<Vec<_>>()));
        ui.set_bt_devices(ModelRc::from(f.model.clone()));
    }
    f.rows = rows;
}

/// Pushes the current state to the panel.
fn render(ui: &AppWindow, f: &mut Feed) {
    let now = Instant::now();
    if f.note.as_ref().is_some_and(|(_, until)| until.is_some_and(|u| u <= now)) {
        f.note = None;
    }
    let prev: Vec<String> = f.rows.iter().map(|r| r.address.clone()).collect();
    let rows = bluetooth::rows(&f.snapshot, f.busy.as_ref().map(|b| b.0.as_str()), &prev);
    apply_rows(ui, f, rows);
    let shown = bluetooth::shown_note(
        f.busy.as_ref().map(|b| b.1.as_str()),
        f.note.as_ref().map(|(n, _)| n),
        f.error.as_deref(),
    );
    let view = bluetooth::view(&f.snapshot, f.scan.remaining_secs(now), shown.as_ref());
    ui.set_bt_state(BtState {
        available: view.available,
        powered: view.powered,
        scanning: view.scanning,
        message: view.message.into(),
        level: view.level,
    });
}

fn render_now(ui: &AppWindow) {
    with(|f| render(ui, f));
}

fn name_of(f: &Feed, addr: &str) -> String {
    f.rows.iter().find(|r| r.address == addr).map_or_else(|| "device".to_string(), |r| r.name.clone())
}

fn set_note(f: &mut Feed, text: String, level: i32, expires: bool) {
    let until = expires.then(|| Instant::now() + Duration::from_secs(NOTE_SECS));
    f.note = Some((Note { text, level }, until));
}

/// Redraws and polls (after something changed).
fn refresh(ui: &AppWindow) {
    render_now(ui);
    poll(ui);
}

/// Something finished: polls that started before this are stale.
fn bump(f: &mut Feed) {
    f.gen += 1;
}

/// Takes a snapshot on a worker and renders it.
fn poll(ui: &AppWindow) {
    let Some((backend, gen)) = with(|f| {
        if !f.active || f.polling {
            return None;
        }
        f.polling = true;
        Some((f.backend.clone(), f.gen))
    })
    .flatten() else {
        return;
    };
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let r = backend.snapshot();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let stale = with(|f| {
                f.polling = false;
                if !f.active {
                    return false;
                }
                // The scan process dying is reported once, whatever else happens.
                if let Some(e) = r.as_ref().ok().and_then(|s| s.scan_error.as_ref()) {
                    eprintln!("[bt] scan process died: {e}");
                    if f.scan.active() {
                        f.scan.stop();
                        set_note(f, format!("Scan stopped: {}", bluetooth::friendly_error(e)), -1, true);
                    }
                }
                if f.gen != gen {
                    return true;
                }
                match r {
                    Ok(s) => {
                        f.snapshot = s;
                        f.error = None;
                    }
                    Err(e) => {
                        eprintln!("[bt] snapshot failed: {e}");
                        f.error = Some(bluetooth::friendly_error(&e));
                    }
                }
                render(&ui, f);
                false
            });
            if stale == Some(true) {
                poll(&ui);
            }
        });
    });
}

/// Every second while the panel shows: end an expired scan, redraw (the
/// countdown) and poll.
fn tick(ui: &AppWindow) {
    let stop = with(|f| {
        let expired = f.scan.take_expired(Instant::now());
        expired.then(|| f.backend.clone())
    })
    .flatten();
    if let Some(backend) = stop {
        println!("[bt] scan ended (60 s)");
        std::thread::spawn(move || {
            let _ = backend.stop_scan();
        });
    }
    refresh(ui);
}

/// The panel opened or closed.
fn changed(ui: &AppWindow, open: bool) {
    if open {
        let first = with(|f| {
            let first = !f.active;
            f.active = true;
            first
        });
        if first != Some(true) {
            return;
        }
        println!("[bt] panel open");
        let weak = ui.as_weak();
        with(|f| {
            f.timer.start(TimerMode::Repeated, Duration::from_secs(1), move || {
                if let Some(ui) = weak.upgrade() {
                    tick(&ui);
                }
            });
        });
        poll(ui);
    } else {
        let stop = with(|f| {
            if !f.active {
                return None;
            }
            f.active = false;
            f.timer.stop();
            let was_scanning = f.scan.active();
            f.scan.stop();
            f.note = None;
            Some((f.backend.clone(), was_scanning))
        })
        .flatten();
        if let Some((backend, was_scanning)) = stop {
            println!("[bt] panel closed{}", if was_scanning { ", scan stopped" } else { "" });
            // Always: a scan may be running that this feed did not time.
            std::thread::spawn(move || {
                let _ = backend.stop_scan();
                backend.close();
            });
        }
    }
}

/// Scan row: start (powering the adapter on first) or stop. "Stop" is offered
/// only for the scan Lumen's own timer runs.
/// Back/Left from the device rows to the panel list: D7 says a scan stops when
/// you leave the device list. Polling goes on while the panel is still shown.
fn leave_rows(ui: &AppWindow) {
    if with(|f| f.scan.active()) == Some(true) {
        scan(ui); // while scanning, the toggle is a stop
    }
}

fn scan(ui: &AppWindow) {
    let Some((backend, scanning, powered)) = with(|f| {
        let powered = f.snapshot.adapter.as_ref().is_some_and(|a| a.powered);
        (f.backend.clone(), powered && f.scan.active(), powered)
    }) else {
        return;
    };
    let weak = ui.as_weak();
    if scanning {
        with(|f| f.scan.stop());
        std::thread::spawn(move || {
            let r = backend.stop_scan();
            let _ = weak.upgrade_in_event_loop(move |ui| {
                with(|f| {
                    bump(f);
                    if let Err(e) = r {
                        set_note(f, bluetooth::friendly_error(&e), -1, true);
                    }
                });
                refresh(&ui);
            });
        });
        render_now(ui);
        return;
    }
    with(|f| {
        f.scan.start(Instant::now(), bluetooth::SCAN_SECS);
        f.note = None;
    });
    render_now(ui);
    std::thread::spawn(move || {
        let r = (|| {
            if !powered {
                backend.set_powered(true)?;
            }
            backend.start_scan(bluetooth::SCAN_SECS)
        })();
        let _ = weak.upgrade_in_event_loop(move |ui| {
            let stopped_meanwhile = with(|f| {
                bump(f);
                if let Err(e) = &r {
                    f.scan.stop();
                    set_note(f, bluetooth::friendly_error(e), -1, true);
                }
                // Closed the panel or pressed Stop while this was starting:
                // the stop may have run first, so stop again.
                r.is_ok() && !f.scan.active()
            });
            if stopped_meanwhile == Some(true) {
                let backend = with(|f| f.backend.clone());
                if let Some(backend) = backend {
                    std::thread::spawn(move || {
                        let _ = backend.stop_scan();
                    });
                }
            }
            refresh(&ui);
        });
    });
}

/// (busy message, success message) for an action; None = unknown action.
fn action_texts(action: &str, name: &str) -> Option<(String, String)> {
    Some(match action {
        "pair" => (format!("Pairing {name}\u{2026}"), format!("Paired {name}")),
        "connect" => (format!("Connecting {name}\u{2026}"), format!("Connected {name}")),
        "disconnect" => (format!("Disconnecting {name}\u{2026}"), format!("Disconnected {name}")),
        "forget" => (format!("Forgetting {name}\u{2026}"), format!("Forgot {name}")),
        _ => return None,
    })
}

/// What `act` found when it looked at the request.
enum Start {
    Unknown,
    Occupied,
    Go(Arc<dyn Bluetooth>, String),
}

/// pair / connect / disconnect / forget on a worker. One at a time.
fn act(ui: &AppWindow, addr: String, action: String) {
    let start = with(|f| {
        let name = name_of(f, &addr);
        let Some((busy_text, ok_text)) = action_texts(&action, &name) else {
            return Start::Unknown;
        };
        if f.busy.is_some() {
            set_note(f, "Another Bluetooth action is running. Wait for it to finish".into(), -1, true);
            return Start::Occupied;
        }
        // Pair/connect stop discovery in the backend (shared radio); end the countdown too.
        let stops_scan = matches!(action.as_str(), "pair" | "connect") && f.scan.active();
        let busy_text = if stops_scan {
            f.scan.stop();
            format!("{busy_text} (scan stopped)")
        } else {
            busy_text
        };
        f.busy = Some((addr.clone(), busy_text));
        f.note = None;
        Start::Go(f.backend.clone(), ok_text)
    });
    render_now(ui);
    let (backend, ok_text) = match start {
        Some(Start::Go(b, t)) => (b, t),
        Some(Start::Unknown) => {
            eprintln!("[bt] unknown action {action:?}");
            return;
        }
        Some(Start::Occupied) | None => return,
    };
    println!("[bt] action {action} {addr}");
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let r = match action.as_str() {
            "pair" => backend.pair(&addr),
            "connect" => backend.connect(&addr),
            "disconnect" => backend.disconnect(&addr),
            "forget" => backend.forget(&addr),
            other => Err(format!("unknown action {other}")),
        };
        let _ = weak.upgrade_in_event_loop(move |ui| {
            with(|f| {
                f.busy = None;
                bump(f);
                match &r {
                    Ok(()) => {
                        println!("[bt] {action} {addr}: ok");
                        set_note(f, ok_text, 1, true);
                    }
                    Err(e) => {
                        println!("[bt] {action} {addr} failed: {e}");
                        set_note(f, bluetooth::friendly_error(e), -1, true);
                    }
                }
            });
            refresh(&ui);
        });
    });
}

pub fn wire(ui: &AppWindow) {
    let model = Rc::new(VecModel::default());
    ui.set_bt_devices(ModelRc::from(model.clone()));
    FEED.with(|c| {
        *c.borrow_mut() = Some(Feed {
            backend: platform::bluetooth(),
            active: false,
            polling: false,
            snapshot: BtSnapshot::default(),
            error: None,
            scan: Default::default(),
            note: None,
            busy: None,
            gen: 0,
            rows: Vec::new(),
            model,
            timer: Timer::default(),
        })
    });
    let weak = ui.as_weak();
    ui.on_bt_changed(move |open| {
        if let Some(ui) = weak.upgrade() {
            changed(&ui, open);
        }
    });
    let weak = ui.as_weak();
    ui.on_bt_leave_rows(move || {
        if let Some(ui) = weak.upgrade() {
            leave_rows(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_bt_scan(move || {
        if let Some(ui) = weak.upgrade() {
            scan(&ui);
        }
    });
    let weak = ui.as_weak();
    ui.on_bt_action(move |addr, action| {
        if let Some(ui) = weak.upgrade() {
            act(&ui, addr.to_string(), action.to_string());
        }
    });
}

#[cfg(test)]
mod tests {
    use super::action_texts;

    #[test]
    fn action_texts_name_the_device_for_each_action() {
        let cases = [
            ("pair", "Pairing Pad\u{2026}", "Paired Pad"),
            ("connect", "Connecting Pad\u{2026}", "Connected Pad"),
            ("disconnect", "Disconnecting Pad\u{2026}", "Disconnected Pad"),
            ("forget", "Forgetting Pad\u{2026}", "Forgot Pad"),
        ];
        for (action, busy, done) in cases {
            assert_eq!(action_texts(action, "Pad"), Some((busy.to_string(), done.to_string())), "{action}");
        }
    }

    #[test]
    fn unknown_actions_have_no_texts() {
        for a in ["", "Pair", "pair ", "remove", "scan"] {
            assert_eq!(action_texts(a, "Pad"), None, "{a:?}");
        }
    }
}
