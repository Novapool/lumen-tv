// Lumen: an Apple TV-style launcher for the Raspberry Pi 5. The home screen
// in Slint, the app registry + lifecycle (launch, track, return), the evdev
// input layer, and the frame-time bench. The store arrives in M4.

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod apps;
mod bench;
mod bake;
mod diag;
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

/// Bakes every app's hero backdrop from its tile gradient (see bake.rs).
fn bake_heroes(ui: &AppWindow, home: &Home) {
    let t = std::time::Instant::now();
    let apps: Vec<App> =
        ui.get_apps().iter().enumerate().map(|(i, app)| App { hero: bake::hero(&home.tints[i]), ..app }).collect();
    ui.set_apps(Rc::new(VecModel::from(apps)).into());
    println!("[bake] heroes in {} ms", t.elapsed().as_millis());
}

/// What the home screen keeps beyond the Slint model, by row i of the model:
/// each tile's gradient (bake input) and icon outcome (`None` for the built-in
/// tiles), plus the registry's load errors. Nothing displays the last two
/// until M3.5's Settings.
struct Home {
    tints: Vec<bake::Tint>,
    #[allow(dead_code)] // M3.5 Settings
    errors: Vec<registry::LoadError>,
    #[allow(dead_code)] // M3.5 Settings
    icons: Vec<Option<icons::IconStatus>>,
}

/// `#rrggbb` (validated by the registry) to rgb.
fn hex_rgb(s: &str) -> [u8; 3] {
    let v = u32::from_str_radix(s.trim_start_matches('#'), 16).unwrap_or(0);
    [(v >> 16) as u8, (v >> 8) as u8, v as u8]
}

/// The 135 degree tile gradient: the stops spread evenly.
fn tile_tint(colors: Option<&[String]>) -> bake::Tint {
    let default = ["#24262c".to_string(), "#6b707c".to_string()];
    let rgb: Vec<[u8; 3]> = colors.unwrap_or(&default).iter().map(|c| hex_rgb(c)).collect();
    bake::Tint::from_stops(135., &rgb)
}

/// Loads the registry (seeding the defaults on first start), decodes the
/// icons and builds the `apps` model: registry entries, then Store and Settings.
fn build_apps() -> (Vec<registry::Entry>, Vec<App>, Home) {
    let dir = registry::apps_dir();
    match registry::seed_defaults(&dir) {
        Ok(true) => println!("[registry] wrote the default apps to {}", dir.display()),
        Ok(false) => {}
        Err(e) => eprintln!("[registry] could not seed {}: {e}", dir.display()),
    }
    let loaded = registry::load(&dir);
    let (config, roots) = (registry::config_dir(), icons::Roots::system());
    let mut apps = Vec::new();
    let mut tints = Vec::new();
    let mut statuses = Vec::new();
    for e in &loaded.entries {
        let status = e.icon.as_deref().map(|spec| icons::for_entry(&e.id, spec, &config, &roots));
        let ready = matches!(status, Some(icons::IconStatus::Ready(..)));
        let glyph = e.glyph.clone().unwrap_or_else(|| e.name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default());
        apps.push(App {
            id: e.id.clone().into(),
            running: false,
            name: e.name.clone().into(),
            glyph: if ready { "".into() } else { glyph.into() },
            hero: Default::default(),
            face: Default::default(),
        });
        tints.push(tile_tint(e.colors.as_deref()));
        statuses.push(status);
    }
    for (id, name, glyph, c0, c1) in [
        ("lumen:store", "Store", "+", "#003b3b", "#19c3b0"),
        ("lumen:settings", "Settings", "⚙", "#24262c", "#6b707c"),
    ] {
        apps.push(App {
            id: id.into(),
            running: false,
            name: name.into(),
            glyph: glyph.into(),
            hero: Default::default(),
            face: Default::default(),
        });
        tints.push(tile_tint(Some(&[c0.to_string(), c1.to_string()])));
        statuses.push(None);
    }
    (loaded.entries, apps, Home { tints, errors: loaded.errors, icons: statuses })
}

/// Bakes the tile faces and the shared shadow for `rem` physical pixels per rem.
fn bake_tiles(ui: &AppWindow, rem: f32, home: &Home) {
    if rem < 1. {
        return; // window not sized yet
    }
    let t = std::time::Instant::now();
    let apps = ui.get_apps();
    for i in 0..apps.row_count() {
        if let Some(app) = apps.row_data(i) {
            let icon = match home.icons.get(i) {
                Some(Some(icons::IconStatus::Ready(icon, _))) => Some(icon),
                _ => None,
            };
            apps.set_row_data(i, App { face: bake::tile_face(&home.tints[i], rem, icon), ..app });
        }
    }
    ui.set_tile_shadow(bake::tile_shadow(rem));
    println!("[bake] tiles at {rem} px/rem in {} ms", t.elapsed().as_millis());
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
    home: Rc<Home>,
    bench: Rc<bench::Bench>,
    status: input::Status,
    debug: Arc<AtomicBool>,
    randr: diag::SharedRandr,
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
        let mode = std::env::var("LUMEN_MODE").ok();
        let perf = self.bench.perf_view();
        let devices = self.status.lock().map(|d| d.clone()).unwrap_or_default();
        let apps = ui.get_apps();
        let icons: Vec<(String, &Option<icons::IconStatus>)> =
            (0..apps.row_count()).filter_map(|i| Some((apps.row_data(i)?.id.to_string(), self.home.icons.get(i)?))).collect();
        vec![
            diag::display_panel(&diag::DisplayIn {
                randr: &randr,
                window: (size.width, size.height, ui.window().scale_factor()),
                lumen_mode: mode.as_deref(),
                perf: &perf,
            }),
            diag::input_panel(&devices, cfg!(target_os = "linux"), self.debug.load(Ordering::Relaxed), diag::claim_status().as_ref(), Instant::now()),
            diag::apps_panel(&self.home.errors, &icons),
            diag::running_panel(self.running().as_ref()),
            diag::log_panel(&logtail::tail(logtail::KEEP)),
        ]
    }
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
        println!("[settings] open: {devices} devices, {} registry errors, {running} running", s.home.errors.len());
        let r = r.clone();
        t.start(TimerMode::Repeated, Duration::from_secs(1), r);
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

fn main() -> Result<(), slint::PlatformError> {
    logtail::install();
    // The compositor tells windows apart by app_id: this is how Lumen finds
    // its own window among the apps' (apps::LUMEN_APP_ID).
    // (set_xdg_app_id needs the platform to exist first, hence select().)
    slint::BackendSelector::new().select()?;
    slint::set_xdg_app_id(apps::LUMEN_APP_ID)?;
    let ui = AppWindow::new()?;
    ui.window().set_fullscreen(true);
    let (entries, model, home) = build_apps();
    ui.set_apps(Rc::new(VecModel::from(model)).into());
    let home = Rc::new(home);
    bake_heroes(&ui, &home);
    ui.on_blend(bake::blend);
    let (weak, rebake_home) = (ui.as_weak(), home.clone());
    ui.on_rebake_tiles(move |rem| {
        if let Some(ui) = weak.upgrade() {
            bake_tiles(&ui, rem, &rebake_home);
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
        Sources { home, bench: bench.clone(), status: input_status, debug: input_debug, randr: Default::default() },
    );
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
