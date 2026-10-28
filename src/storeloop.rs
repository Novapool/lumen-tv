// `lumen-tv --store-loop [--source <manifest>] [--dry-run] [ids...]`: the M5.5
// Gate's automated loop. For each selected user-level (non-root) entry:
// install through the Store's own job runner, check the registry TOML and
// receipt appeared, launch it the way a tile does, wait for a fullscreen
// window with the entry's app_id, close it, wait for the exit, uninstall,
// check the TOML and receipt (and the Flatpak) are gone.
//
// Root entries are skipped (they need the PIN pad). `--dry-run` prints every
// command and runs nothing. One PASS/FAIL/SKIP line per entry; exit 1 on any
// FAIL. Run it from a shell inside the Lumen session (or with the session
// variables set); the windows it opens are not Lumen's own launches.

use std::cell::RefCell;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::apps;
use crate::platform::{self, Toplevel, Windows};
use crate::registry::{Entry, Kind};
use crate::store::{self, Action, Dirs, JobEvent, Limits, Root, StoreApp};

/// How long the entry's window gets to appear fullscreen.
const WINDOW_WAIT: Duration = Duration::from_secs(30);
/// How long a closed app gets to exit before it is killed.
const EXIT_WAIT: Duration = Duration::from_secs(10);

// ---- arguments ----

#[derive(Debug, Default, PartialEq, Eq)]
pub struct Opts {
    pub source: Option<String>,
    pub dry_run: bool,
    pub ids: Vec<String>,
}

pub fn parse_args(args: &[String]) -> Result<Opts, String> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--dry-run" => o.dry_run = true,
            "--source" => o.source = Some(it.next().ok_or("--source needs a manifest path or URL")?.clone()),
            s if s.starts_with("--") => return Err(format!("unknown option {s}")),
            id => o.ids.push(id.to_string()),
        }
    }
    Ok(o)
}

// ---- selection ----

#[derive(Debug, Default)]
pub struct Plan<'a> {
    pub run: Vec<&'a StoreApp>,
    /// (id, reason)
    pub skipped: Vec<(String, String)>,
    /// Requested ids that aren't in the manifest.
    pub unknown: Vec<String>,
}

/// Why the loop won't run this entry, if it won't.
fn skip_reason(app: &StoreApp, arch: &str, installed: &dyn Fn(&StoreApp) -> bool) -> Option<String> {
    if app.root {
        return Some("root app: needs the PIN pad, install it by hand".into());
    }
    if !app.tile {
        return Some("no window: background service, checked at the TV".into());
    }
    if !app.runs_on(arch) {
        return Some(format!("not for this machine ({arch}; entry lists {})", app.arch.join(", ")));
    }
    let needs_privilege = |steps: &[String]| steps.iter().any(|s| s.split_whitespace().any(|w| w == "sudo"));
    if needs_privilege(&app.install) || needs_privilege(&app.uninstall) {
        return Some("a step needs root".into());
    }
    if installed(app) {
        return Some("already installed: uninstall it first or pass ids".into());
    }
    None
}

/// `ids` empty = every entry. Order follows the manifest. `installed` says
/// whether an entry is already on the machine (those are skipped, never touched).
pub fn select<'a>(apps: &'a [StoreApp], ids: &[String], arch: &str, installed: &dyn Fn(&StoreApp) -> bool) -> Plan<'a> {
    let mut plan = Plan::default();
    plan.unknown = ids.iter().filter(|i| !apps.iter().any(|a| a.id() == i.as_str())).cloned().collect();
    for app in apps.iter().filter(|a| ids.is_empty() || ids.iter().any(|i| i == a.id())) {
        match skip_reason(app, arch, installed) {
            Some(why) => plan.skipped.push((app.id().to_string(), why)),
            None => plan.run.push(app),
        }
    }
    plan
}

// ---- helpers ----

/// The Flatpak ref a step list installs or removes: the last positional word
/// of its `flatpak install|uninstall` step (a leading remote name such as
/// `flathub` is a positional too, so the last one is the app).
pub fn flatpak_ref(steps: &[String]) -> Option<String> {
    steps.iter().find_map(|s| {
        let mut w = s.split_whitespace();
        if w.next()? != "flatpak" {
            return None;
        }
        let verb = w.next()?;
        if verb != "install" && verb != "uninstall" {
            return None;
        }
        w.filter(|x| !x.starts_with('-')).last().map(str::to_string)
    })
}

/// The app_id the window will carry: the entry's, or `lumen-<id>` for a web app.
pub fn expected_app_id(e: &Entry) -> Option<String> {
    e.app_id.clone().or_else(|| (e.kind == Kind::Url).then(|| format!("lumen-{}", e.id)))
}

/// A window of `app_id` that wasn't there before the launch (`ignore`).
pub fn find_window(tops: &[Toplevel], app_id: &str, ignore: &[u64]) -> Option<Toplevel> {
    tops.iter().find(|t| t.app_id.eq_ignore_ascii_case(app_id) && !ignore.contains(&t.id)).cloned()
}

// ---- results ----

#[derive(Debug)]
pub struct Outcome {
    pub id: String,
    /// Err((stage, reason))
    pub result: Result<(), (&'static str, String)>,
    pub timings: Vec<(&'static str, Duration)>,
}

fn secs(d: Duration) -> String {
    format!("{:.1}s", d.as_secs_f64())
}

pub fn format_outcome(o: &Outcome) -> String {
    let times: Vec<String> = o.timings.iter().map(|(k, d)| format!("{k} {}", secs(*d))).collect();
    let times = times.join(", ");
    match &o.result {
        Ok(()) => format!("PASS {}  ({times})", o.id),
        Err((stage, why)) => format!("FAIL {}  at {stage}: {why}  ({times})", o.id),
    }
}

pub fn format_summary(pass: usize, fail: usize, skip: usize) -> String {
    format!("{pass} pass, {fail} fail, {skip} skipped")
}

// ---- dry run ----

fn print_plan(dirs: &Dirs, plan: &Plan, source: &str) {
    println!("dry run: nothing below is executed (source {source}, arch {})", store::running_arch());
    for app in &plan.run {
        let e = &app.entry;
        println!("== {} ({})", app.id(), e.name);
        if app.install.is_empty() {
            println!("  install:   (no commands, only the app entry)");
        }
        for s in &app.install {
            println!("  install:   {s}");
        }
        println!("  expect:    {}/{}.toml and {}/{}.json", dirs.registry.display(), app.id(), dirs.receipts.display(), app.id());
        match apps::command_line(e, &apps::browser_cmd(), &|_| true) {
            Ok(line) => println!("  launch:    XDG_RUNTIME_DIR=/run/user/1000 WAYLAND_DISPLAY=wayland-0 sh -c 'exec {line}'"),
            Err(why) => println!("  launch:    (cannot build a launch line: {why})"),
        }
        match expected_app_id(e) {
            Some(id) => println!("  window:    toplevel app_id={id} fullscreen within {} s", WINDOW_WAIT.as_secs()),
            None => println!("  window:    NO app_id: the real run would FAIL here"),
        }
        println!("  close:     ask the window to close, wait up to {} s for the launcher to exit, then kill its group", EXIT_WAIT.as_secs());
        if app.uninstall.is_empty() {
            println!("  uninstall: (no commands, only the app entry)");
        }
        for s in &app.uninstall {
            println!("  uninstall: {s}");
        }
        println!("  expect:    the .toml and the receipt are gone");
        if let Some(r) = flatpak_ref(&app.install) {
            println!("  expect:    `flatpak info --user {r}` fails");
        }
    }
    for (id, why) in &plan.skipped {
        println!("SKIP {id}  {why}");
    }
}

// ---- the real loop ----

fn flatpak_present(r: &str) -> bool {
    Command::new("flatpak")
        .args(["info", "--user", r])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

/// True when the registry entry, the receipt or the Flatpak already exists.
fn already_installed(dirs: &Dirs, app: &StoreApp) -> bool {
    let (toml, receipt) = files_state(dirs, app.id());
    toml || receipt || flatpak_ref(&app.install).is_some_and(|r| flatpak_present(&r))
}

fn files_state(dirs: &Dirs, id: &str) -> (bool, bool) {
    (dirs.registry.join(format!("{id}.toml")).exists(), dirs.receipts.join(format!("{id}.json")).exists())
}

/// Runs one Store job on this thread, echoing its steps. Err = the failure text.
fn job(dirs: &Dirs, source: &str, action: Action, app: &StoreApp, wait: &[u32]) -> Result<(), String> {
    let failure: RefCell<Option<String>> = RefCell::new(None);
    let helper = platform::root_installer();
    let root = Root { helper: helper.as_ref(), pin: None };
    let on_event = |ev: JobEvent| match ev {
        JobEvent::Step { k, n, command } => println!("    [{}] step {k}/{n}: {command}", action.as_str()),
        JobEvent::Failed { message, .. } => *failure.borrow_mut() = Some(message),
        _ => {}
    };
    let limits = Limits::DEFAULT;
    store::run_job_blocking(dirs, source, action, app, wait, limits, root, &on_event)
        .map_err(|e| failure.borrow().clone().unwrap_or(e))
}

#[cfg(target_os = "linux")]
fn signal_group(sig: i32, pid: u32) {
    if pid > 1 {
        unsafe { libc::kill(-(pid as i32), sig) };
    }
}

// dash rejects `kill -TERM -- -<pgid>` ("Illegal number"); `-s` works in every sh.
#[cfg(not(target_os = "linux"))]
fn signal_group(sig: &str, pid: u32) {
    let _ = Command::new("sh")
        .args(["-c", &format!("kill -s {sig} -- -{pid}")])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn kill_group(pid: u32) {
    #[cfg(target_os = "linux")]
    let (term, kill) = (libc::SIGTERM, libc::SIGKILL);
    #[cfg(not(target_os = "linux"))]
    let (term, kill) = ("TERM", "KILL");
    signal_group(term, pid);
    std::thread::sleep(Duration::from_secs(2));
    signal_group(kill, pid);
}

/// Session variables a launched app needs; set only when the shell hasn't.
fn session_env() {
    for (k, v) in [("XDG_RUNTIME_DIR", "/run/user/1000"), ("WAYLAND_DISPLAY", "wayland-0"), ("DISPLAY", ":0")] {
        if std::env::var_os(k).is_none_or(|x| x.is_empty()) {
            std::env::set_var(k, v);
        }
    }
}

#[cfg(target_os = "linux")]
fn connect_windows() -> Result<Arc<dyn Windows>, String> {
    platform::toplevel::connect(|_| {})
}

#[cfg(not(target_os = "linux"))]
fn connect_windows() -> Result<Arc<dyn Windows>, String> {
    Err("window checks need labwc (Linux)".into())
}

/// Remember the app_ids of new windows that are not the one we expect.
fn note_others(others: &mut Vec<String>, tops: &[Toplevel], app_id: &str, before: &[u64]) {
    for w in tops.iter().filter(|w| !before.contains(&w.id) && !w.app_id.eq_ignore_ascii_case(app_id)) {
        if !others.contains(&w.app_id) {
            others.push(w.app_id.clone());
        }
    }
}

fn seen_ids(others: &[String]) -> String {
    if others.is_empty() {
        " (no new windows appeared)".into()
    } else {
        format!(" (other new window app_ids seen: {}; check the entry's app_id)", others.iter().map(|o| format!("{o:?}")).collect::<Vec<_>>().join(", "))
    }
}

/// Failure text when the launcher exits before a fullscreen window was found.
fn exited_message(code: i32, app_id: &str, seen: Option<&Toplevel>, others: &[String]) -> String {
    match seen {
        Some(w) => format!("the app exited (code {code}); a window with app_id {:?} appeared but fullscreen=false", w.app_id),
        None => format!("the app exited (code {code}) before any window with app_id {app_id} appeared{}", seen_ids(others)),
    }
}

fn run_one(dirs: &Dirs, source: &str, app: &StoreApp, win: &Arc<dyn Windows>) -> Outcome {
    let id = app.id().to_string();
    let mut timings: Vec<(&'static str, Duration)> = Vec::new();
    let fail = |timings: Vec<(&'static str, Duration)>, stage, why: String| Outcome { id: id.clone(), result: Err((stage, why)), timings };

    // Already-installed entries were skipped in `select`; this is the plain-data part.
    let fp_install = flatpak_ref(&app.install);
    let fp_remove = flatpak_ref(&app.uninstall).or_else(|| fp_install.clone());
    let Some(app_id) = expected_app_id(&app.entry) else {
        return fail(timings, "precheck", "the entry has no app_id to look for".into());
    };

    // Everything after the install must end in an uninstall, pass or fail.
    let cleanup = |why: Option<String>, mut timings: Vec<(&'static str, Duration)>, stage: &'static str| {
        let t = Instant::now();
        let r = job(dirs, source, Action::Uninstall, app, &[]);
        timings.push(("uninstall", t.elapsed()));
        let gone = || -> Result<(), String> {
            r.clone()?;
            let (toml, receipt) = files_state(dirs, &id);
            if toml || receipt {
                return Err(format!("after uninstall: {}{}still there", if toml { "registry entry " } else { "" }, if receipt { "receipt " } else { "" }));
            }
            if let Some(rf) = &fp_remove {
                if flatpak_present(rf) {
                    return Err(format!("`flatpak info --user {rf}` still succeeds after uninstall"));
                }
            }
            Ok(())
        };
        match (why, gone()) {
            (None, Ok(())) => Outcome { id: id.clone(), result: Ok(()), timings },
            (None, Err(e)) => Outcome { id: id.clone(), result: Err(("uninstall", e)), timings },
            (Some(w), Ok(())) => Outcome { id: id.clone(), result: Err((stage, w)), timings },
            (Some(w), Err(e)) => Outcome { id: id.clone(), result: Err((stage, format!("{w}; cleanup also failed: {e}"))), timings },
        }
    };

    // Install, then the files must exist.
    let t = Instant::now();
    let r = job(dirs, source, Action::Install, app, &[]);
    timings.push(("install", t.elapsed()));
    if let Err(e) = r {
        let leftover = fp_install.as_ref().is_some_and(|r| flatpak_present(r));
        let note = if leftover { format!("; the Flatpak is partly installed, remove it with: {}", app.uninstall.join(" && ")) } else { String::new() };
        return fail(timings, "install", format!("{e}{note}"));
    }
    let (toml, receipt) = files_state(dirs, &id);
    if !(toml && receipt) {
        let why = format!("after install: missing {}{}", if toml { "" } else { "registry entry " }, if receipt { "" } else { "receipt" });
        return cleanup(Some(why), timings, "files");
    }

    // Launch the way a tile does.
    let before: Vec<u64> = win.toplevels().iter().map(|t| t.id).collect();
    let (tx, rx) = mpsc::channel();
    let t = Instant::now();
    let pid = match apps::spawn(&app.entry, &apps::browser_cmd(), move |_, pid, code| {
        let _ = tx.send((pid, code));
    }) {
        Ok(p) => p,
        Err(e) => return cleanup(Some(e), timings, "launch"),
    };
    println!("    launched pid {pid}, waiting up to {} s for a fullscreen window app_id={app_id}", WINDOW_WAIT.as_secs());

    // Window: present and fullscreen.
    let mut seen: Option<Toplevel> = None;
    let mut found: Option<Toplevel> = None;
    let mut others: Vec<String> = Vec::new();
    while t.elapsed() < WINDOW_WAIT {
        if let Some(w) = find_window(&win.toplevels(), &app_id, &before) {
            seen = Some(w.clone());
            if w.fullscreen {
                found = Some(w);
                break;
            }
        } else {
            note_others(&mut others, &win.toplevels(), &app_id, &before);
            if let Ok((_, code)) = rx.try_recv() {
                timings.push(("launch->window", t.elapsed()));
                return cleanup(Some(exited_message(code, &app_id, seen.as_ref(), &others)), timings, "window");
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    timings.push(("launch->window", t.elapsed()));
    let mut why = None;
    let mut stage = "window";
    match (&found, &seen) {
        (Some(_), _) => {}
        (None, Some(w)) => why = Some(format!("window {:?} (app_id {app_id}) appeared but never went fullscreen in {} s", w.title, WINDOW_WAIT.as_secs())),
        (None, None) => why = Some(format!("no window with app_id {app_id} in {} s{}", WINDOW_WAIT.as_secs(), seen_ids(&others))),
    }

    // Close, wait for the exit; kill the group if it lingers.
    let t = Instant::now();
    let target = found.or(seen);
    if let Some(w) = &target {
        if let Err(e) = win.close(w.id) {
            println!("    close request failed: {e}");
        }
    }
    let mut exited = rx.recv_timeout(EXIT_WAIT).is_ok() || store::wait_for_exit(&id, &[pid], Duration::from_millis(1));
    if !exited {
        println!("    still running after {} s, killing its process group", EXIT_WAIT.as_secs());
        kill_group(pid);
        exited = store::wait_for_exit(&id, &[pid], Duration::from_secs(3));
        if why.is_none() {
            why = Some("the window did not close the app in time (had to kill it)".into());
            stage = "close";
        }
    }
    timings.push(("close", t.elapsed()));
    if !exited && why.is_none() {
        why = Some("the app would not exit".into());
        stage = "close";
    }
    cleanup(why, timings, stage)
}

/// Entry point for `--store-loop`.
pub fn run(args: &[String]) -> ! {
    let opts = match parse_args(args) {
        Ok(o) => o,
        Err(e) => {
            eprintln!("store-loop: {e}\nusage: lumen-tv --store-loop [--source <manifest>] [--dry-run] [ids...]");
            std::process::exit(2);
        }
    };
    let source = opts.source.clone().unwrap_or_else(store::source);
    println!("source: {source}");
    let manifest = match store::load_manifest(&source) {
        Ok(m) => m,
        Err(e) => {
            println!("manifest error: {e}");
            std::process::exit(1);
        }
    };
    for e in &manifest.errors {
        println!("manifest: skipped {e}");
    }
    let dirs = Dirs::default_dirs();
    let plan = select(&manifest.apps, &opts.ids, &store::running_arch(), &|a| already_installed(&dirs, a));
    if !plan.unknown.is_empty() {
        println!("unknown id(s): {}", plan.unknown.join(", "));
        std::process::exit(2);
    }
    if opts.dry_run {
        print_plan(&dirs, &plan, &source);
        println!("{} to run, {} skipped (dry run, nothing executed)", plan.run.len(), plan.skipped.len());
        std::process::exit(0);
    }

    session_env();
    let win = match connect_windows() {
        Ok(w) => w,
        Err(e) => {
            println!("cannot watch windows: {e} (run this from the Lumen session: XDG_RUNTIME_DIR, WAYLAND_DISPLAY)");
            std::process::exit(1);
        }
    };
    println!("registry {}  receipts {}", dirs.registry.display(), dirs.receipts.display());
    let (mut pass, mut fail) = (0, 0);
    for app in &plan.run {
        println!("== {}", app.id());
        let o = run_one(&dirs, &source, app, &win);
        println!("{}", format_outcome(&o));
        if o.result.is_ok() {
            pass += 1;
        } else {
            fail += 1;
        }
    }
    for (id, why) in &plan.skipped {
        println!("SKIP {id}  {why}");
    }
    println!("{}", format_summary(pass, fail, plan.skipped.len()));
    std::process::exit(i32::from(fail > 0));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app(id: &str, root: bool, arch: &[&str], install: &[&str]) -> StoreApp {
        let v = serde_json::json!({
            "id": id, "name": id, "type": "command", "launch": format!("{id} --fullscreen"),
            "description": "d", "install": install, "uninstall": [], "root": root,
        });
        let mut a = store::parse_app(v).unwrap();
        a.arch = arch.iter().map(|s| s.to_string()).collect();
        a
    }

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn selection_skips_root_and_other_arch() {
        let apps = vec![
            app("a", false, &[], &[]),
            app("b", true, &[], &[]),
            app("c", false, &["x86_64"], &[]),
            app("d", false, &["aarch64", "x86_64"], &[]),
            app("e", false, &[], &["sudo apt install -y x"]),
        ];
        let p = select(&apps, &[], "aarch64", &|_| false);
        assert_eq!(p.run.iter().map(|a| a.id()).collect::<Vec<_>>(), ["a", "d"]);
        let skipped: Vec<_> = p.skipped.iter().map(|(i, _)| i.as_str()).collect();
        assert_eq!(skipped, ["b", "c", "e"]);
        assert!(p.skipped[0].1.contains("root"));
        assert!(p.skipped[1].1.contains("x86_64"));
    }

    #[test]
    fn selection_honours_ids_and_reports_unknown() {
        let apps = vec![app("a", false, &[], &[]), app("b", false, &[], &[]), app("r", true, &[], &[])];
        let p = select(&apps, &s(&["b", "r", "zzz"]), "aarch64", &|_| false);
        assert_eq!(p.run.iter().map(|a| a.id()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.unknown, ["zzz"]);
    }

    #[test]
    fn selection_skips_tile_less_entries() {
        let mut bg = app("bg", false, &[], &[]);
        bg.tile = false;
        let apps = vec![app("a", false, &[], &[]), bg];
        let p = select(&apps, &[], "aarch64", &|_| false);
        assert_eq!(p.run.iter().map(|a| a.id()).collect::<Vec<_>>(), ["a"]);
        assert_eq!(p.skipped.len(), 1);
        assert!(p.skipped[0].1.contains("background service"));
    }

    #[test]
    fn selection_skips_already_installed() {
        let apps = vec![app("a", false, &[], &[]), app("b", false, &[], &[])];
        let p = select(&apps, &[], "aarch64", &|x| x.id() == "a");
        assert_eq!(p.run.iter().map(|a| a.id()).collect::<Vec<_>>(), ["b"]);
        assert_eq!(p.skipped.len(), 1);
        assert_eq!(p.skipped[0].0, "a");
        assert!(p.skipped[0].1.contains("already installed"));
    }

    #[test]
    fn exit_messages_distinguish_cases() {
        let w = Toplevel { id: 1, app_id: "vlc".into(), title: String::new(), activated: false, fullscreen: false };
        let m = exited_message(0, "vlc", Some(&w), &[]);
        assert!(m.contains("\"vlc\"") && m.contains("fullscreen=false"));
        let m = exited_message(1, "vlc", None, &["org.videolan.VLC".into()]);
        assert!(m.contains("before any window") && m.contains("org.videolan.VLC"));
        assert!(exited_message(1, "vlc", None, &[]).contains("no new windows"));
        let mut o = Vec::new();
        let t = |id, a: &str| Toplevel { id, app_id: a.into(), title: String::new(), activated: false, fullscreen: true };
        note_others(&mut o, &[t(1, "old"), t(2, "VLC"), t(3, "x"), t(4, "x")], "vlc", &[1]);
        assert_eq!(o, ["x"]);
    }

    #[test]
    fn flatpak_ref_takes_the_app_not_the_remote() {
        let i = s(&["flatpak install --user -y --noninteractive flathub org.libretro.RetroArch"]);
        assert_eq!(flatpak_ref(&i).as_deref(), Some("org.libretro.RetroArch"));
        let u = s(&["flatpak uninstall --user -y org.videolan.VLC"]);
        assert_eq!(flatpak_ref(&u).as_deref(), Some("org.videolan.VLC"));
        assert_eq!(flatpak_ref(&s(&["curl -L x | sh"])), None);
        assert_eq!(flatpak_ref(&[]), None);
    }

    #[test]
    fn expected_app_id_defaults_for_web_apps() {
        let mut a = app("x", false, &[], &[]);
        assert_eq!(expected_app_id(&a.entry), None);
        a.entry.app_id = Some("retroarch".into());
        assert_eq!(expected_app_id(&a.entry).as_deref(), Some("retroarch"));
        a.entry.app_id = None;
        a.entry.kind = Kind::Url;
        assert_eq!(expected_app_id(&a.entry).as_deref(), Some("lumen-x"));
    }

    #[test]
    fn window_matching_ignores_old_windows_and_case() {
        let t = |id, app: &str| Toplevel { id, app_id: app.into(), title: String::new(), activated: false, fullscreen: true };
        let tops = vec![t(1, "retroarch"), t(2, "RetroArch")];
        assert_eq!(find_window(&tops, "retroarch", &[]).unwrap().id, 1);
        assert_eq!(find_window(&tops, "retroarch", &[1]).unwrap().id, 2);
        assert!(find_window(&tops, "retroarch", &[1, 2]).is_none());
        assert!(find_window(&tops, "vlc", &[]).is_none());
    }

    #[test]
    fn args_parse() {
        let o = parse_args(&s(&["--dry-run", "--source", "m.json", "vlc", "retroarch"])).unwrap();
        assert_eq!(o, Opts { source: Some("m.json".into()), dry_run: true, ids: s(&["vlc", "retroarch"]) });
        assert!(parse_args(&s(&["--source"])).is_err());
        assert!(parse_args(&s(&["--nope"])).is_err());
        assert_eq!(parse_args(&[]).unwrap(), Opts::default());
    }

    #[test]
    fn outcome_lines() {
        let pass = Outcome { id: "vlc".into(), result: Ok(()), timings: vec![("install", Duration::from_millis(12340)), ("close", Duration::from_millis(500))] };
        assert_eq!(format_outcome(&pass), "PASS vlc  (install 12.3s, close 0.5s)");
        let bad = Outcome { id: "vlc".into(), result: Err(("window", "no window".into())), timings: vec![("install", Duration::from_secs(2))] };
        assert_eq!(format_outcome(&bad), "FAIL vlc  at window: no window  (install 2.0s)");
        assert_eq!(format_summary(3, 1, 2), "3 pass, 1 fail, 2 skipped");
    }
}
