// Lumen: an Apple TV-style launcher for the Raspberry Pi 5. The home screen
// in Slint, the app registry + lifecycle (launch, track, return), the evdev
// input layer, and the frame-time bench. The store arrives in M4.

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod apps;
mod bench;
mod bake;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod bluetooth;
mod btfeed;
mod diag;
mod home;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod logtail;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod reclaim;
mod registry;
mod icons;
// Input and platform code runs on the Pi (Linux) only; elsewhere, e.g. UI
// work on the Mac, the window's own key events drive navigation.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod input;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod platform;
#[cfg(target_os = "linux")]
mod session;
mod settings;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod store;
mod storefeed;

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Model, ModelRc, SharedString, Timer, TimerMode, VecModel};

slint::include_modules!();

fn time_now() -> String {
    chrono::Local::now().format("%-I:%M %p").to_string()
}

/// Panels as the Slint model rows: the outer model is created once, and an
/// update only touches the rows that changed, so the open screen keeps its
/// scroll position and focus across the 1 s refresh.
#[derive(Default)]
struct Feed {
    last: Vec<diag::Panel>,
    outer: Rc<VecModel<DiagPanel>>,
    inner: Vec<Rc<VecModel<DiagRow>>>,
}

fn slint_row(r: &diag::Row) -> DiagRow {
    DiagRow { text: r.text.as_str().into(), detail: r.detail.as_str().into(), level: r.level }
}

impl Feed {
    fn apply(&mut self, ui: &AppWindow, panels: Vec<diag::Panel>) {
        let same_shape = self.last.len() == panels.len() && self.last.iter().zip(&panels).all(|(a, b)| a.title == b.title);
        if !same_shape {
            self.inner = panels.iter().map(|p| Rc::new(VecModel::from(p.rows.iter().map(slint_row).collect::<Vec<_>>()))).collect();
            let rows: Vec<DiagPanel> = panels
                .iter()
                .zip(&self.inner)
                .map(|(p, m)| DiagPanel { title: SharedString::from(p.title), rows: ModelRc::from(m.clone()) })
                .collect();
            self.outer.set_vec(rows);
            ui.set_diag_panels(ModelRc::from(self.outer.clone()));
        } else {
            for (i, p) in panels.iter().enumerate() {
                if self.last[i] == *p {
                    continue;
                }
                let model = &self.inner[i];
                if model.row_count() == p.rows.len() {
                    for (j, r) in p.rows.iter().enumerate() {
                        if self.last[i].rows.get(j) != Some(r) {
                            model.set_row_data(j, slint_row(r));
                        }
                    }
                } else {
                    model.set_vec(p.rows.iter().map(slint_row).collect::<Vec<_>>());
                }
            }
        }
        self.last = panels;
    }
}

/// Everything Settings reads, gathered on the UI thread.
struct Sources {
    bench: Rc<bench::Bench>,
    status: input::Status,
    debug: Arc<AtomicBool>,
    randr: diag::SharedRandr,
    /// The last settings.toml parse error logged, so a broken file logs once
    /// rather than on every 1 s refresh.
    logged_settings_err: RefCell<Option<String>>,
}

impl Sources {
    fn running(&self) -> Option<diag::RunningView> {
        #[cfg(target_os = "linux")]
        return session::running_view();
        #[cfg(not(target_os = "linux"))]
        None
    }

    fn panels(&self, ui: &AppWindow) -> Vec<diag::Panel> {
        let size = ui.window().size();
        let randr = self.randr.lock().map(|r| r.clone()).unwrap_or_default();
        let settings_path_buf = settings::path();
        let (loaded, settings_error) = settings::load();
        if *self.logged_settings_err.borrow() != settings_error {
            if let Some(e) = &settings_error {
                eprintln!("[settings] {e}");
            }
            *self.logged_settings_err.borrow_mut() = settings_error.clone();
        }
        let (mode, mode_origin) = settings::resolve_mode(&loaded, std::env::var("LUMEN_MODE").ok().as_deref());
        let status = platform::output::status();
        let (detail, level) = diag::resolution_detail(&status);
        ui.set_resolution_detail(detail.into());
        ui.set_resolution_level(level);
        let cfg_dir = settings_path_buf.parent().map(std::path::Path::to_path_buf).unwrap_or_default();
        let mappings = diag::mapping_sources(&input::SHIPPED, diag::read_user_mappings(&cfg_dir.join("input")));
        let (store_url, store_origin) =
            settings::resolve_store_url(&loaded, std::env::var("LUMEN_STORE_URL").ok().as_deref(), store::DEFAULT_URL);
        let host = host_name();
        let settings_path = {
            let full = settings_path_buf.display().to_string();
            match std::env::var("HOME").ok().filter(|h| !h.is_empty()).and_then(|h| full.strip_prefix(&h).map(str::to_string)) {
                Some(rest) => format!("~{rest}"),
                None => full,
            }
        };
        let perf = self.bench.perf_view();
        let devices = self.status.lock().map(|d| d.clone()).unwrap_or_default();
        let apps = ui.get_apps();
        vec![
            diag::display_panel(&diag::DisplayIn {
                randr: &randr,
                window: (size.width, size.height, ui.window().scale_factor()),
                saved_mode: (&mode, mode_origin),
                perf: &perf,
            }),
            diag::input_panel(&devices, cfg!(target_os = "linux"), self.debug.load(Ordering::Relaxed), diag::claim_status().as_ref(), Instant::now()),
            diag::mappings_panel(&mappings),
            home::with(|h| {
                let icons: Vec<(String, &Option<icons::IconStatus>)> =
                    (0..apps.row_count()).filter_map(|i| Some((apps.row_data(i)?.id.to_string(), h.icons.get(i)?))).collect();
                diag::apps_panel(&h.errors, &icons)
            }),
            diag::running_panel(self.running().as_ref()),
            diag::about_panel(&diag::AboutIn {
                git: env!("LUMEN_GIT"),
                build_date: env!("LUMEN_BUILD_DATE"),
                host: host.as_deref(),
                uptime: uptime(),
                store_url: &store_url,
                store_origin,
                settings_error: settings_error.as_deref(),
                settings_path: &settings_path,
            }),
            diag::log_panel(&logtail::tail(logtail::KEEP)),
        ]
    }
}

fn host_name() -> Option<String> {
    let name = std::fs::read_to_string("/etc/hostname")
        .ok()
        .or_else(|| std::process::Command::new("hostname").output().ok().map(|o| String::from_utf8_lossy(&o.stdout).into_owned()))?;
    Some(name.trim().to_string()).filter(|n| !n.is_empty())
}

/// Host uptime from /proc/uptime (None off Linux).
fn uptime() -> Option<Duration> {
    let text = std::fs::read_to_string("/proc/uptime").ok()?;
    text.split_whitespace().next()?.parse::<f64>().ok().map(Duration::from_secs_f64)
}

/// Settings feed: refresh `diag-panels` every second while the screen is open
/// (no timer while closed) and wire the input-debug toggle.
fn wire_settings(ui: &AppWindow, src: Sources) {
    ui.set_input_debug_on(src.debug.load(Ordering::Relaxed));
    let src = Rc::new(src);
    let feed = Rc::new(RefCell::new(Feed::default()));
    let timer = Rc::new(Timer::default());

    let refresh = {
        let (src, feed, weak) = (src.clone(), feed.clone(), ui.as_weak());
        move || {
            if let Some(ui) = weak.upgrade() {
                feed.borrow_mut().apply(&ui, src.panels(&ui));
            }
        }
    };

    let (s, t, r) = (src.clone(), timer.clone(), refresh.clone());
    ui.on_settings_changed(move |open| {
        if !open {
            t.stop();
            println!("[settings] closed");
            return;
        }
        diag::refresh_randr(&s.randr);
        r();
        let running = s.running().map_or(0, |v| v.apps.len());
        let devices = s.status.lock().map_or(0, |d| d.len());
        println!("[settings] open: {devices} devices, {} registry errors, {running} running", home::with(|h| h.errors.len()));
        let r = r.clone();
        t.start(TimerMode::Repeated, Duration::from_secs(1), r);
    });

    // Resolution: flip between the two saved modes, remember it, and tell the
    // output watcher (which also re-applies it when the TV comes back).
    let (weak, r) = (ui.as_weak(), refresh.clone());
    ui.on_toggle_resolution(move || {
        let cur = platform::output::status().desired;
        let next = if cur == settings::MODES[1].1 { settings::MODES[0].1 } else { settings::MODES[1].1 };
        println!("[settings] resolution {cur} -> {next}");
        let saved = settings::save_mode(next);
        if let Err(e) = &saved {
            println!("[settings] could not save mode: {e}");
        }
        platform::output::request_mode(next);
        if let Some(ui) = weak.upgrade() {
            if ui.get_settings_open() {
                r();
                // The refresh wrote the watcher's detail; a failed save replaces it.
                if let Err(e) = saved {
                    ui.set_resolution_detail(format!("{} · not saved: {e}", settings::mode_label(next)).into());
                    ui.set_resolution_level(diag::ERROR);
                }
            }
        }
    });

    let (weak, s, r) = (ui.as_weak(), src, refresh);
    ui.on_toggle_input_debug(move || {
        let on = !s.debug.fetch_xor(true, Ordering::Relaxed);
        println!("[settings] input debug {}", if on { "on" } else { "off" });
        if let Some(ui) = weak.upgrade() {
            ui.set_input_debug_on(on);
            if !on {
                ui.set_input_debug("".into());
            }
            if ui.get_settings_open() {
                r();
            }
        }
    });
}

/// `lumen-tv --store-check [source]`: fetch + parse the manifest, print it, and
/// exit 0 if there are no errors, else 1. No UI.
fn store_check(source: Option<String>) -> ! {
    let source = source.unwrap_or_else(store::source);
    println!("source: {source}");
    let text = match store::fetch(&source) {
        Ok(t) => t,
        Err(e) => {
            println!("fetch error: {e}");
            std::process::exit(1);
        }
    };
    let m = store::parse_manifest(&text);
    for a in &m.apps {
        println!("{} ({}){}", a.id(), a.entry.name, if a.root { " [root]" } else { "" });
        for s in &a.install {
            println!("  install:   {s}");
        }
        for s in &a.uninstall {
            println!("  uninstall: {s}");
        }
    }
    for e in &m.errors {
        println!("error: {e}");
    }
    println!("{} apps, {} errors{}", m.apps.len(), m.errors.len(), if m.fatal { " (fatal)" } else { "" });
    std::process::exit(i32::from(!m.errors.is_empty() || m.fatal));
}

/// `lumen-tv --bt-check`: one read-only snapshot (the panel's code path);
/// prints the adapter and every device, then the rows the panel would show.
/// Exit 0 if BlueZ answered, else 1. No UI, nothing is scanned or changed.
fn bt_check() -> ! {
    let backend = platform::bluetooth();
    match backend.snapshot() {
        Ok(s) => {
            match &s.adapter {
                Some(a) => println!("adapter: {} powered={} discovering={}", a.address, a.powered, a.discovering),
                None => println!("adapter: none"),
            }
            for d in &s.devices {
                println!(
                    "device: {} name={:?} icon={:?} paired={} connected={} rssi={:?} battery={:?}",
                    d.address, d.name, d.icon, d.paired, d.connected, d.rssi, d.battery
                );
            }
            let rows = bluetooth::rows(&s, None, &[]);
            println!("{} devices, {} shown", s.devices.len(), rows.len());
            std::process::exit(i32::from(s.adapter.is_none()));
        }
        Err(e) => {
            println!("snapshot error: {e}");
            println!("({})", bluetooth::friendly_error(&e));
            std::process::exit(1);
        }
    }
}

fn main() -> Result<(), slint::PlatformError> {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--store-check") => store_check(args.next()),
        Some("--bt-check") => bt_check(),
        _ => {}
    }
    logtail::install();
    platform::output::start(settings::mode().0);
    // The compositor tells windows apart by app_id: this is how Lumen finds
    // its own window among the apps' (apps::LUMEN_APP_ID).
    // (set_xdg_app_id needs the platform to exist first, hence select().)
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id(apps::LUMEN_APP_ID)?;
    let ui = AppWindow::new()?;
    ui.window().set_fullscreen(true);
    let entries = home::init(&ui);
    ui.on_blend(bake::blend);
    let weak = ui.as_weak();
    ui.on_rebake_tiles(move |rem| {
        if let Some(ui) = weak.upgrade() {
            home::bake_tiles(&ui, rem);
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

    // Select: a short press-in on the focused tile (launch(i) does the rest).
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

    // The input layer fills `input_status`; the toggle flips `input_debug`
    // (LUMEN_INPUT_DEBUG=1 is only its starting value).
    let input_status = input::Status::default();
    let input_debug = Arc::new(AtomicBool::new(std::env::var("LUMEN_INPUT_DEBUG").is_ok_and(|v| v == "1")));

    // One human line per navigation step (focus, row, button), only while input debug is on.
    let debug = input_debug.clone();
    ui.on_nav_trace(move |line| {
        if debug.load(Ordering::Relaxed) {
            println!("[nav] {line}");
        }
    });

    #[cfg(target_os = "linux")]
    {
        let front = session::start(&ui, entries);
        start_input(&ui, front, input_status.clone(), input_debug.clone());
    }
    #[cfg(not(target_os = "linux"))]
    {
        let weak = ui.as_weak();
        ui.on_launch(move |i| {
            let id = weak.upgrade().and_then(|ui| ui.get_apps().row_data(i as usize)).map(|a| a.id);
            println!("[apps] launch {:?} (no launching off the Pi)", id.unwrap_or_default());
        });
        let _ = entries;
    }

    let bench = bench::Bench::attach(&ui);
    wire_settings(
        &ui,
        Sources { bench: bench.clone(), status: input_status, debug: input_debug, randr: Default::default(), logged_settings_err: RefCell::new(None) },
    );
    storefeed::wire(&ui);
    btfeed::wire(&ui);
    let weak = ui.as_weak();
    let b = bench.clone();
    ui.on_bench(move || {
        if let Some(ui) = weak.upgrade() {
            b.run(&ui);
        }
    });

    // LUMEN_BENCH=1 (in ~/.config/lumen-tv/env, which lumen-test.service loads)
    // starts the bench 3 s after launch. LUMEN_BENCH=bt does it with Settings
    // open on the Bluetooth panel (rows alternate Down/Up).
    match std::env::var("LUMEN_BENCH").as_deref() {
        Ok("1") => {
            let weak = ui.as_weak();
            Timer::single_shot(Duration::from_secs(3), move || {
                if let Some(ui) = weak.upgrade() {
                    bench.run(&ui);
                }
            });
        }
        Ok("bt") => {
            let weak = ui.as_weak();
            Timer::single_shot(Duration::from_secs(3), move || {
                if let Some(ui) = weak.upgrade() {
                    if bench::open_bluetooth_panel(&ui) {
                        bench.run_screen(&ui, bench::Screen::Bluetooth);
                    } else {
                        println!("[perf] bt bench: could not open the Bluetooth panel");
                    }
                }
            });
        }
        _ => {}
    }

    let result = ui.run();
    logtail::shutdown();
    result
}

/// Starts the evdev input layer and the TV remote, and hands navigation to them.
#[cfg(target_os = "linux")]
fn start_input(ui: &AppWindow, front: Arc<AtomicBool>, status: input::Status, debug: Arc<AtomicBool>) {
    use input::Action;

    session::claim_remote_at_startup();

    let weak = ui.as_weak();
    input::start(status, debug.clone(), move |action, device| {
        let debug = debug.load(Ordering::Relaxed);
        // While an app is in front only Home is Lumen's: the app gets the rest.
        let lumen_front = front.load(Ordering::Relaxed);
        if !lumen_front && action != Action::Home {
            if debug {
                println!("[input] {action:?} from \"{device}\" ignored (app in front)");
            }
            return;
        }
        if debug {
            println!("[input] {action:?} from \"{device}\"");
        }
        let label = debug.then(|| format!("{action:?} · {device}"));
        let _ = weak.upgrade_in_event_loop(move |ui| {
            if action == Action::Home {
                session::home();
            }
            if action == Action::Home && !lumen_front {
                return; // the jump to the first tile is for Home pressed on Lumen
            }
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
