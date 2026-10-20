// The Settings data feed: pure builders from Lumen's live state to the five
// Rust-fed panels (Display, Input, Apps, Running, Log; the Toggles panel is
// built by the UI). main.rs turns `Panel`s into the Slint `DiagPanel`s.
// Everything here is platform independent and unit tested; the Linux-only
// sources (input status, window list, wlr-randr) are passed in as plain data.

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::apps::{Foreground, Snapshot};
use crate::bench::PerfView;
use crate::icons::IconStatus;
use crate::input::DeviceStatus;
use crate::platform::Toplevel;
use crate::registry::LoadError;

pub const ERROR: i32 = -1;
pub const NORMAL: i32 = 0;
pub const GOOD: i32 = 1;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Row {
    pub text: String,
    pub detail: String,
    pub level: i32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Panel {
    pub title: &'static str,
    pub rows: Vec<Row>,
}

fn row(text: impl Into<String>, detail: impl Into<String>, level: i32) -> Row {
    Row {
        text: text.into(),
        detail: detail.into(),
        level,
    }
}

// ---------------------------------------------------------------- display

/// One output from `wlr-randr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub name: String,
    pub enabled: bool,
    /// Current mode, "1920x1080 @ 60.00 Hz".
    pub mode: Option<String>,
    pub scale: Option<String>,
}

/// Parses `wlr-randr` (human readable) output.
pub fn parse_wlr_randr(text: &str) -> Vec<Output> {
    let mut out: Vec<Output> = Vec::new();
    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        if !line.starts_with(char::is_whitespace) {
            let name = line.split_once(' ').map_or(line, |(n, _)| n);
            out.push(Output {
                name: name.to_string(),
                enabled: true,
                mode: None,
                scale: None,
            });
            continue;
        }
        let Some(cur) = out.last_mut() else { continue };
        let t = line.trim();
        if let Some(v) = t.strip_prefix("Enabled:") {
            cur.enabled = v.trim() == "yes";
        } else if let Some(v) = t.strip_prefix("Scale:") {
            cur.scale = Some(v.trim().to_string());
        } else if t.contains(" px,") && t.contains("current") {
            // "1920x1080 px, 60.000000 Hz (preferred, current)"
            let mut it = t.split_whitespace();
            if let (Some(res), Some(_px), Some(hz)) = (it.next(), it.next(), it.next()) {
                let hz: f64 = hz.parse().unwrap_or(0.0);
                cur.mode = Some(format!("{res} @ {hz:.2} Hz"));
            }
        }
    }
    out
}

/// Where the wlr-randr query stands.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Randr {
    #[default]
    NotAsked,
    Pending,
    Ready(Vec<Output>),
    /// Not a Linux build: there is no wlr-randr to ask.
    Unavailable,
    Failed(String),
}

pub type SharedRandr = Arc<Mutex<Randr>>;

/// Runs `wlr-randr` on a background thread, publishing into `slot`.
pub fn refresh_randr(slot: &SharedRandr) {
    if !cfg!(target_os = "linux") {
        *slot.lock().unwrap() = Randr::Unavailable;
        return;
    }
    *slot.lock().unwrap() = Randr::Pending;
    let slot = slot.clone();
    std::thread::spawn(move || {
        let res = match std::process::Command::new("wlr-randr").output() {
            Ok(o) if o.status.success() => {
                Randr::Ready(parse_wlr_randr(&String::from_utf8_lossy(&o.stdout)))
            }
            Ok(o) => Randr::Failed(format!("wlr-randr exited with {}", o.status)),
            Err(e) => Randr::Failed(format!("wlr-randr: {e}")),
        };
        *slot.lock().unwrap() = res;
    });
}

pub struct DisplayIn<'a> {
    pub randr: &'a Randr,
    /// Lumen's own window, physical px, and its scale factor.
    pub window: (u32, u32, f32),
    pub lumen_mode: Option<&'a str>,
    pub perf: &'a PerfView,
}

pub fn display_panel(d: &DisplayIn) -> Panel {
    let mut rows = Vec::new();
    match d.randr {
        Randr::NotAsked | Randr::Pending => rows.push(row("Outputs", "reading wlr-randr…", NORMAL)),
        Randr::Unavailable => rows.push(row("Outputs", "n/a", NORMAL)),
        Randr::Failed(why) => rows.push(row("Outputs", why.clone(), ERROR)),
        Randr::Ready(outs) if outs.is_empty() => rows.push(row("Outputs", "none reported", ERROR)),
        Randr::Ready(outs) => {
            for o in outs {
                let mode = o.mode.clone().unwrap_or_else(|| {
                    if o.enabled {
                        "no current mode".into()
                    } else {
                        "disabled".into()
                    }
                });
                let scale = o
                    .scale
                    .as_deref()
                    .map(|s| format!(" · scale {s}"))
                    .unwrap_or_default();
                rows.push(row(
                    o.name.clone(),
                    format!("{mode}{scale}"),
                    if o.mode.is_some() { GOOD } else { NORMAL },
                ));
            }
        }
    }
    let (w, h, scale) = d.window;
    rows.push(if w == 0 || h == 0 {
        row("Lumen window", "n/a", NORMAL)
    } else {
        row(
            "Lumen window",
            format!("{w}x{h} px · scale {scale}"),
            NORMAL,
        )
    });
    rows.push(row(
        "LUMEN_MODE",
        d.lumen_mode.unwrap_or("not set (default 1080p60)"),
        NORMAL,
    ));
    rows.push(match d.perf.fps {
        Some((f, age)) if age.as_secs() < 2 => row(
            "Frame rate",
            format!("{f:.1} fps (while animating)"),
            NORMAL,
        ),
        Some((f, age)) => row(
            "Frame rate",
            format!("{f:.1} fps (while animating, {} s ago)", age.as_secs()),
            NORMAL,
        ),
        None => row("Frame rate", "no animation yet", NORMAL),
    });
    rows.push(match &d.perf.last {
        Some((pass, text)) => row(
            "Last benchmark",
            text.clone(),
            if *pass { GOOD } else { ERROR },
        ),
        None => row("Last benchmark", "not run since start", NORMAL),
    });
    Panel {
        title: "Display",
        rows,
    }
}

// ------------------------------------------------------------------ input

pub fn ago(d: Duration) -> String {
    match d.as_secs() {
        0..=1 => "just now".into(),
        s @ 2..=119 => format!("{s} s ago"),
        s => format!("{} min ago", s / 60),
    }
}

fn short_path(p: &str) -> &str {
    p.rsplit('/').next().unwrap_or(p)
}

/// Result of the last remote claim (Ok = status string, Err = failure), set by
/// the claim thread and shown in the Input panel.
static CLAIM: std::sync::Mutex<Option<Result<String, String>>> = std::sync::Mutex::new(None);

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn set_claim(r: Result<String, String>) {
    if let Ok(mut c) = CLAIM.lock() {
        *c = Some(r);
    }
}

pub fn claim_status() -> Option<Result<String, String>> {
    CLAIM.lock().ok().and_then(|c| c.clone())
}

pub fn input_panel(
    devices: &[DeviceStatus],
    available: bool,
    debug_on: bool,
    claim: Option<&Result<String, String>>,
    now: Instant,
) -> Panel {
    let mut rows = Vec::new();
    if !available {
        rows.push(row(
            "Input devices",
            "n/a (evdev input runs on the Pi only)",
            NORMAL,
        ));
        return Panel {
            title: "Input",
            rows,
        };
    }
    match claim {
        Some(Ok(status)) => rows.push(row(
            "TV remote claim",
            status.clone(),
            if status.contains("FAILED") {
                ERROR
            } else {
                GOOD
            },
        )),
        Some(Err(e)) => rows.push(row("TV remote claim", format!("failed: {e}"), ERROR)),
        None => rows.push(row("TV remote claim", "not yet claimed", NORMAL)),
    }
    if devices.is_empty() {
        rows.push(row("Input devices", "none open", ERROR));
    }
    for d in devices {
        let mut detail = match &d.mapping {
            Some(file) => format!("{} → {file}", short_path(&d.path)),
            None => format!("{} → no mapping, ignored", short_path(&d.path)),
        };
        match &d.last_action {
            Some((action, at)) => detail.push_str(&format!(
                " · last {action:?} {}",
                ago(now.saturating_duration_since(*at))
            )),
            None if d.mapping.is_some() => detail.push_str(" · no input yet"),
            None => {}
        }
        if debug_on {
            match &d.last_raw {
                Some((key, at)) => detail.push_str(&format!(
                    " · raw {key} {}",
                    ago(now.saturating_duration_since(*at))
                )),
                None => detail.push_str(" · raw: none yet"),
            }
        }
        rows.push(row(
            d.name.clone(),
            detail,
            if d.mapping.is_some() { GOOD } else { NORMAL },
        ));
    }
    Panel {
        title: "Input",
        rows,
    }
}

// ------------------------------------------------------------------- apps

/// `icons`: (app id, outcome) for every tile; built-in tiles have `None`.
pub fn apps_panel(errors: &[LoadError], icons: &[(String, &Option<IconStatus>)]) -> Panel {
    let mut rows = Vec::new();
    let with_icon = icons.iter().filter(|(_, s)| s.is_some()).count();
    rows.push(row(
        "Registry",
        format!(
            "{} tiles · {} with icons · {} file errors",
            icons.len(),
            with_icon,
            errors.len()
        ),
        if errors.is_empty() { GOOD } else { ERROR },
    ));
    for e in errors {
        rows.push(row(e.file.clone(), e.message.clone(), ERROR));
    }
    for (id, status) in icons {
        match status {
            Some(IconStatus::Missing(why)) => {
                rows.push(row(format!("{id} icon"), why.clone(), ERROR))
            }
            Some(IconStatus::Ready(_, path)) => {
                rows.push(row(format!("{id} icon"), path.display().to_string(), GOOD))
            }
            None => {}
        }
    }
    Panel {
        title: "Apps",
        rows,
    }
}

// ---------------------------------------------------------------- running

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowView {
    pub id: u64,
    pub app_id: String,
    pub activated: bool,
    pub fullscreen: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningApp {
    pub id: String,
    pub pid: Option<u32>,
    pub launching: bool,
    pub windows: Vec<WindowView>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunningView {
    pub foreground: Foreground,
    pub apps: Vec<RunningApp>,
}

/// The lifecycle snapshot joined with the compositor's windows.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn join_running(snap: &Snapshot, toplevels: &[Toplevel]) -> RunningView {
    RunningView {
        foreground: snap.foreground.clone(),
        apps: snap
            .apps
            .iter()
            .map(|a| RunningApp {
                id: a.id.clone(),
                pid: a.pid,
                launching: a.launching,
                windows: a
                    .windows
                    .iter()
                    .map(|w| match toplevels.iter().find(|t| t.id == *w) {
                        Some(t) => WindowView {
                            id: t.id,
                            app_id: t.app_id.clone(),
                            activated: t.activated,
                            fullscreen: t.fullscreen,
                        },
                        None => WindowView {
                            id: *w,
                            app_id: "?".into(),
                            activated: false,
                            fullscreen: false,
                        },
                    })
                    .collect(),
            })
            .collect(),
    }
}

pub fn running_panel(view: Option<&RunningView>) -> Panel {
    let mut rows = Vec::new();
    let Some(v) = view else {
        rows.push(row(
            "Apps",
            "n/a (window control runs on the Pi only)",
            NORMAL,
        ));
        return Panel {
            title: "Running",
            rows,
        };
    };
    rows.push(row(
        "In front",
        match &v.foreground {
            Foreground::Lumen => "Lumen".to_string(),
            Foreground::App(id) => id.clone(),
        },
        NORMAL,
    ));
    if v.apps.is_empty() {
        rows.push(row("Nothing running", "", NORMAL));
    }
    for a in &v.apps {
        let pid = a.pid.map_or("no pid".to_string(), |p| format!("pid {p}"));
        let state = if a.launching {
            "launching, no window yet"
        } else {
            "running"
        };
        rows.push(row(
            a.id.clone(),
            format!("{pid} · {state}"),
            if a.launching { NORMAL } else { GOOD },
        ));
        for w in &a.windows {
            let mut flags = Vec::new();
            if w.activated {
                flags.push("activated");
            }
            if w.fullscreen {
                flags.push("fullscreen");
            }
            rows.push(row(
                format!("  window {}", w.id),
                format!(
                    "{} · {}",
                    w.app_id,
                    if flags.is_empty() {
                        "background".into()
                    } else {
                        flags.join(", ")
                    }
                ),
                NORMAL,
            ));
        }
    }
    Panel {
        title: "Running",
        rows,
    }
}

// -------------------------------------------------------------------- log

pub fn log_panel(lines: &[String]) -> Panel {
    let rows = if lines.is_empty() {
        vec![row("No log lines captured", "", NORMAL)]
    } else {
        lines
            .iter()
            .map(|l| {
                let low = l.to_ascii_lowercase();
                let bad =
                    low.contains("error") || low.contains("failed") || low.contains("panicked");
                row(l.clone(), "", if bad { ERROR } else { NORMAL })
            })
            .collect()
    };
    Panel { title: "Log", rows }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::apps::AppSnapshot;
    use std::path::PathBuf;

    const RANDR: &str = r#"HDMI-A-1 "LG Electronics LG TV SSCR2 0x01010101 (HDMI-A-1)"
  Physical size: 1600x900 mm
  Enabled: yes
  Modes:
    3840x2160 px, 60.000000 Hz (preferred)
    1920x1080 px, 59.940060 Hz (current)
    1280x720 px, 60.000000 Hz
  Position: 0,0
  Transform: normal
  Scale: 1.000000
HDMI-A-2 "Unplugged"
  Enabled: no
  Modes:
    640x480 px, 60.000000 Hz (preferred)
"#;

    #[test]
    fn parses_wlr_randr() {
        let outs = parse_wlr_randr(RANDR);
        assert_eq!(outs.len(), 2);
        assert_eq!(outs[0].name, "HDMI-A-1");
        assert_eq!(outs[0].mode.as_deref(), Some("1920x1080 @ 59.94 Hz"));
        assert_eq!(outs[0].scale.as_deref(), Some("1.000000"));
        assert!(!outs[1].enabled);
        assert_eq!(outs[1].mode, None);
        assert!(parse_wlr_randr("").is_empty());
        assert!(parse_wlr_randr("  stray indented line\n").is_empty());
    }

    fn perf() -> PerfView {
        PerfView::default()
    }

    #[test]
    fn display_rows_cover_each_randr_state() {
        let p = perf();
        let mk = |randr: &Randr| {
            display_panel(&DisplayIn {
                randr,
                window: (1920, 1080, 1.0),
                lumen_mode: None,
                perf: &p,
            })
        };
        assert_eq!(mk(&Randr::Pending).rows[0].detail, "reading wlr-randr…");
        assert_eq!(mk(&Randr::Unavailable).rows[0].level, NORMAL);
        assert_eq!(
            mk(&Randr::Failed("wlr-randr: not found".into())).rows[0].level,
            ERROR
        );
        let ready = mk(&Randr::Ready(parse_wlr_randr(RANDR)));
        assert_eq!(ready.rows[0].text, "HDMI-A-1");
        assert!(ready.rows[0].detail.starts_with("1920x1080 @ 59.94 Hz"));
        assert_eq!(ready.rows[0].level, GOOD);
        assert_eq!(ready.rows[1].detail, "disabled");
        assert!(ready
            .rows
            .iter()
            .any(|r| r.text == "Lumen window" && r.detail == "1920x1080 px · scale 1"));
        assert!(ready
            .rows
            .iter()
            .any(|r| r.text == "LUMEN_MODE" && r.detail.starts_with("not set")));
        assert!(ready
            .rows
            .iter()
            .any(|r| r.text == "Last benchmark" && r.detail == "not run since start"));
    }

    #[test]
    fn display_shows_fps_and_last_bench() {
        let p = PerfView {
            fps: Some((60.0, Duration::ZERO)),
            last: Some((false, "FAIL · 40 fps".into())),
        };
        let d = display_panel(&DisplayIn {
            randr: &Randr::NotAsked,
            window: (0, 0, 1.0),
            lumen_mode: Some("3840x2160@60"),
            perf: &p,
        });
        assert!(d
            .rows
            .iter()
            .any(|r| r.text == "Frame rate" && r.detail.starts_with("60.0 fps")));
        assert!(d
            .rows
            .iter()
            .any(|r| r.text == "Last benchmark" && r.level == ERROR));
        assert!(d
            .rows
            .iter()
            .any(|r| r.text == "Lumen window" && r.detail == "n/a"));
        assert!(d
            .rows
            .iter()
            .any(|r| r.text == "LUMEN_MODE" && r.detail == "3840x2160@60"));
    }

    #[test]
    fn ago_scales() {
        assert_eq!(ago(Duration::from_millis(400)), "just now");
        assert_eq!(ago(Duration::from_secs(5)), "5 s ago");
        assert_eq!(ago(Duration::from_secs(300)), "5 min ago");
    }

    fn dev(name: &str, mapping: Option<&str>) -> DeviceStatus {
        DeviceStatus {
            path: format!("/dev/input/{name}"),
            name: name.into(),
            mapping: mapping.map(Into::into),
            last_action: None,
            last_raw: None,
        }
    }

    #[test]
    fn input_rows() {
        let now = Instant::now();
        assert_eq!(
            input_panel(&[], false, false, None, now).rows[0].level,
            NORMAL
        );
        let none = input_panel(&[], true, false, None, now);
        assert_eq!(
            (none.rows[0].level, none.rows[0].detail.as_str()),
            (NORMAL, "not yet claimed")
        );
        assert_eq!(none.rows[1].level, ERROR);
        let ok = Ok("Playback \"Lumen\" at 2.0.0.0, remap ok".to_string());
        assert_eq!(
            input_panel(&[], true, false, Some(&ok), now).rows[0].level,
            GOOD
        );
        let bad = Ok("x, remap FAILED: denied".to_string());
        assert_eq!(
            input_panel(&[], true, false, Some(&bad), now).rows[0].level,
            ERROR
        );
        let err = Err("no HDMI physical address".to_string());
        assert_eq!(
            input_panel(&[], true, false, Some(&err), now).rows[0].level,
            ERROR
        );
        let mut remote = dev("event3", Some("cec-remote.toml"));
        remote.last_action = Some((crate::input::Action::Select, now));
        remote.last_raw = Some(("KEY_OK".into(), now));
        let mouse = dev("event4", None);
        let off = input_panel(
            &[remote.clone(), mouse.clone()],
            true,
            false,
            None,
            now + Duration::from_secs(3),
        );
        assert_eq!(off.rows[0].text, "TV remote claim");
        assert_eq!(
            off.rows[1].detail,
            "event3 → cec-remote.toml · last Select 3 s ago"
        );
        assert_eq!(off.rows[1].level, GOOD);
        assert_eq!(off.rows[2].detail, "event4 → no mapping, ignored");
        assert_eq!(off.rows[2].level, NORMAL);
        let on = input_panel(&[remote, mouse], true, true, None, now);
        assert!(on.rows[1].detail.ends_with("raw KEY_OK just now"));
        assert!(on.rows[2].detail.ends_with("raw: none yet"));
    }

    #[test]
    fn apps_rows() {
        let errs = vec![LoadError {
            file: "bad.toml".into(),
            message: "missing name".into(),
        }];
        let miss = Some(IconStatus::Missing("not found in hicolor".into()));
        let none: Option<IconStatus> = None;
        let icons = vec![
            ("kodi".to_string(), &miss),
            ("lumen:store".to_string(), &none),
        ];
        let p = apps_panel(&errs, &icons);
        assert_eq!(p.rows[0].detail, "2 tiles · 1 with icons · 1 file errors");
        assert_eq!(p.rows[0].level, ERROR);
        assert_eq!(p.rows[1], row("bad.toml", "missing name", ERROR));
        assert_eq!(p.rows[2], row("kodi icon", "not found in hicolor", ERROR));
        assert_eq!(p.rows.len(), 3);
        let clean = apps_panel(&[], &icons[1..]);
        assert_eq!(clean.rows[0].level, GOOD);
        let _ = PathBuf::new();
    }

    fn top(id: u64, app_id: &str, activated: bool) -> Toplevel {
        Toplevel {
            id,
            app_id: app_id.into(),
            title: String::new(),
            activated,
            fullscreen: activated,
        }
    }

    #[test]
    fn joins_snapshot_with_toplevels() {
        let snap = Snapshot {
            foreground: Foreground::App("kodi".into()),
            apps: vec![
                AppSnapshot {
                    id: "kodi".into(),
                    pid: Some(42),
                    windows: vec![7, 9],
                    launching: false,
                },
                AppSnapshot {
                    id: "jf".into(),
                    pid: None,
                    windows: vec![],
                    launching: true,
                },
            ],
        };
        let v = join_running(&snap, &[top(7, "kodi", true)]);
        assert_eq!(
            v.apps[0].windows[0],
            WindowView {
                id: 7,
                app_id: "kodi".into(),
                activated: true,
                fullscreen: true
            }
        );
        assert_eq!(v.apps[0].windows[1].app_id, "?");
        let p = running_panel(Some(&v));
        assert_eq!(p.rows[0], row("In front", "kodi", NORMAL));
        assert_eq!(p.rows[1].detail, "pid 42 · running");
        assert_eq!(p.rows[2].detail, "kodi · activated, fullscreen");
        assert_eq!(p.rows[3].detail, "? · background");
        assert_eq!(p.rows[4].detail, "no pid · launching, no window yet");
    }

    #[test]
    fn running_empty_and_unavailable() {
        assert_eq!(running_panel(None).rows[0].level, NORMAL);
        assert!(running_panel(None).rows[0].detail.starts_with("n/a"));
        let v = RunningView {
            foreground: Foreground::Lumen,
            apps: vec![],
        };
        let p = running_panel(Some(&v));
        assert_eq!(p.rows[0].detail, "Lumen");
        assert_eq!(p.rows[1].text, "Nothing running");
    }

    #[test]
    fn log_rows_flag_errors() {
        let p = log_panel(&[
            "[apps] spawn kodi".into(),
            "[windows] activate: Failed to x".into(),
        ]);
        assert_eq!(p.rows[0].level, NORMAL);
        assert_eq!(p.rows[1].level, ERROR);
        assert_eq!(log_panel(&[]).rows[0].text, "No log lines captured");
    }
}
