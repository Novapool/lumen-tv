// Store: manifest fetch + parse, install/uninstall runner, receipts. See AGENT_JOBS.md.
//
// Pure logic, no Slint: the UI glue (storefeed.rs) calls `load_manifest`,
// `uninstall_steps` and `run_job`. Steps run as `sh -c '<step>'` as
// the current user. `root: true` apps are never run (Lumen never calls sudo),
// and there is no rollback: a failed job leaves whatever the steps did, but
// writes no registry entry and removes none.

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::registry::{self, Entry};

pub const DEFAULT_URL: &str = "https://raw.githubusercontent.com/Novapool/lumen-tv/main/store/manifest.json";

/// Keys of a manifest app that aren't registry fields.
const STORE_KEYS: [&str; 5] = ["description", "detail", "install", "uninstall", "root"];

/// One app in the manifest: the registry entry it would install, plus store-only data.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreApp {
    pub entry: Entry,
    pub description: String,
    pub detail: Option<String>,
    pub install: Vec<String>,
    pub uninstall: Vec<String>,
    pub root: bool,
}

impl StoreApp {
    pub fn id(&self) -> &str {
        &self.entry.id
    }

    /// The manifest-format JSON object for this app (also what a receipt stores).
    pub fn to_value(&self) -> Value {
        let mut v = serde_json::to_value(&self.entry).expect("Entry serializes");
        let o = v.as_object_mut().expect("Entry is an object");
        o.insert("description".into(), json!(self.description));
        if let Some(d) = &self.detail {
            o.insert("detail".into(), json!(d));
        }
        o.insert("install".into(), json!(self.install));
        o.insert("uninstall".into(), json!(self.uninstall));
        if self.root {
            o.insert("root".into(), json!(true));
        }
        v
    }
}

#[derive(Debug, Default)]
pub struct Manifest {
    pub apps: Vec<StoreApp>,
    /// Skipped apps (and fatal problems), one message each.
    pub errors: Vec<String>,
    /// True when the whole document was unusable (bad JSON, wrong version).
    pub fatal: bool,
}

fn parse_steps(o: &mut serde_json::Map<String, Value>, key: &str) -> Result<Vec<String>, String> {
    let v = o.remove(key).ok_or_else(|| format!("missing \"{key}\" (use [] for none)"))?;
    let Value::Array(items) = v else {
        return Err(format!("\"{key}\" should be a list of strings"));
    };
    items
        .into_iter()
        .map(|s| match s {
            Value::String(s) if !s.trim().is_empty() => Ok(s),
            _ => Err(format!("\"{key}\" steps should be non-empty strings")),
        })
        .collect()
}

/// Parses one manifest app object (also used for receipts).
pub fn parse_app(v: Value) -> Result<StoreApp, String> {
    let Value::Object(mut o) = v else {
        return Err("app should be an object".into());
    };
    let install = parse_steps(&mut o, "install")?;
    let uninstall = parse_steps(&mut o, "uninstall")?;
    let description = match o.remove("description") {
        None => String::new(),
        Some(Value::String(s)) => s,
        Some(_) => return Err("\"description\" should be a string".into()),
    };
    let detail = match o.remove("detail") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s),
        Some(_) => return Err("\"detail\" should be a string".into()),
    };
    let root = match o.remove("root") {
        None => false,
        Some(Value::Bool(b)) => b,
        Some(_) => return Err("\"root\" should be true or false".into()),
    };
    debug_assert!(STORE_KEYS.iter().all(|k| !o.contains_key(*k)));
    let entry: Entry = serde_json::from_value(Value::Object(o)).map_err(|e| e.to_string())?;
    entry.validate()?;
    Ok(StoreApp { entry, description, detail, install, uninstall, root })
}

/// Parses a manifest document. A bad app is skipped and recorded in `errors`.
pub fn parse_manifest(text: &str) -> Manifest {
    let mut m = Manifest::default();
    let fatal = |m: &mut Manifest, msg: String| {
        m.errors.push(msg);
        m.fatal = true;
    };
    let doc: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => {
            fatal(&mut m, format!("manifest is not valid JSON: {e}"));
            return m;
        }
    };
    match doc.get("version").and_then(Value::as_i64) {
        Some(1) => {}
        other => {
            fatal(&mut m, format!("unsupported manifest version {}", other.map_or("(none)".into(), |v| v.to_string())));
            return m;
        }
    }
    let Some(apps) = doc.get("apps").and_then(Value::as_array) else {
        fatal(&mut m, "manifest has no \"apps\" list".into());
        return m;
    };
    let mut seen = HashSet::new();
    for (i, v) in apps.iter().enumerate() {
        let label = v.get("id").and_then(Value::as_str).map_or(format!("app #{}", i + 1), str::to_string);
        match parse_app(v.clone()) {
            Ok(app) if !seen.insert(app.id().to_string()) => m.errors.push(format!("{label}: duplicate id")),
            Ok(app) => m.apps.push(app),
            Err(e) => m.errors.push(format!("{label}: {e}")),
        }
    }
    m
}

/// `LUMEN_STORE_URL`, else the public manifest.
pub fn source() -> String {
    std::env::var("LUMEN_STORE_URL").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_URL.to_string())
}

/// Blocking: reads a local path / `file://` URL, or runs `curl -fsSL --max-time 20`.
pub fn fetch(source: &str) -> Result<String, String> {
    if source.starts_with('/') || source.starts_with("file://") {
        let path = source.strip_prefix("file://").unwrap_or(source);
        return std::fs::read_to_string(path).map_err(|e| format!("can't read {path}: {e}"));
    }
    let out = Command::new("curl")
        .args(["-fsSL", "--max-time", "20", source])
        .stdin(Stdio::null())
        .output()
        .map_err(|e| format!("can't run curl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        let err = err.trim();
        return Err(if err.is_empty() { format!("curl failed ({})", out.status) } else { err.to_string() });
    }
    String::from_utf8(out.stdout).map_err(|_| "manifest is not valid UTF-8".to_string())
}

/// Fetch + parse, with the `[store] manifest` log lines. Blocking.
pub fn load_manifest(source: &str) -> Result<Manifest, String> {
    let result = fetch(source).and_then(|text| {
        let m = parse_manifest(&text);
        if m.fatal {
            Err(m.errors.join("; "))
        } else {
            Ok(m)
        }
    });
    match &result {
        Ok(m) => {
            eprintln!("[store] manifest: {} apps, {} errors from {source}", m.apps.len(), m.errors.len());
            for e in &m.errors {
                eprintln!("[store] manifest: skipped {e}");
            }
        }
        Err(e) => eprintln!("[store] manifest error: {e}"),
    }
    result
}

// ---- receipts ----

/// A receipt keeps the installed app's manifest object, so an uninstall still
/// works if the manifest changes. It also records where and when (for people
/// reading the file); only `app` is read back.
pub fn receipts_dir() -> PathBuf {
    registry::config_dir().join("store")
}

pub fn write_receipt(dir: &Path, app: &StoreApp, source: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(format!("{}.json", app.id()));
    let v = json!({
        "source": source,
        "installed_at": chrono::Utc::now().to_rfc3339(),
        "app": app.to_value(),
    });
    let text = serde_json::to_string_pretty(&v).map_err(std::io::Error::other)?;
    let tmp = dir.join(format!(".{}.json.tmp", app.id()));
    std::fs::write(&tmp, text + "\n")?;
    std::fs::rename(&tmp, &path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })?;
    Ok(path)
}

/// The app a receipt recorded, if the file is readable.
pub fn read_receipt(dir: &Path, id: &str) -> Option<StoreApp> {
    let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).ok()?;
    let v: Value = serde_json::from_str(&text).ok()?;
    parse_app(v.get("app")?.clone()).ok()
}

pub fn remove_receipt(dir: &Path, id: &str) -> std::io::Result<bool> {
    match std::fs::remove_file(dir.join(format!("{id}.json"))) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(e),
    }
}

/// Every readable receipt in `dir`, by id.
pub fn read_receipts(dir: &Path) -> HashMap<String, StoreApp> {
    let mut out = HashMap::new();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(id) = name.strip_suffix(".json").filter(|i| !i.starts_with('.')) {
                if let Some(r) = read_receipt(dir, id) {
                    out.insert(id.to_string(), r);
                }
            }
        }
    }
    out
}

/// The steps an uninstall would really run: the receipt's if there is one.
pub fn uninstall_steps(app: &StoreApp, receipt: Option<&StoreApp>) -> Vec<String> {
    receipt.map_or_else(|| app.uninstall.clone(), |r| r.uninstall.clone())
}

// ---- runner ----

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Install,
    Uninstall,
}

impl Action {
    pub fn as_str(self) -> &'static str {
        match self {
            Action::Install => "install",
            Action::Uninstall => "uninstall",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JobEvent {
    /// Step `k` of `n` (1-based) is starting.
    Step { k: usize, n: usize, command: String },
    /// A new output line.
    Line(String),
    /// Replaces the previous line (the text before it ended in a bare `\r`: progress bars).
    Replace(String),
    Done,
    /// `code` is the exit status, -1 if the step was killed or didn't start (0 when
    /// the steps passed but writing the registry entry failed).
    Failed { k: usize, code: i32, message: String },
}

/// Splits a byte stream into lines on `\n` and `\r`. A segment that follows a
/// `\r`-terminated one replaces it instead of adding a line.
#[derive(Default)]
struct LineSplitter {
    buf: Vec<u8>,
    after_cr: bool,
}

impl LineSplitter {
    fn emit(&mut self, out: &mut Vec<JobEvent>, terminator: Option<u8>) {
        let text = String::from_utf8_lossy(&self.buf).trim_end().to_string();
        self.buf.clear();
        if text.is_empty() {
            if terminator == Some(b'\n') {
                self.after_cr = false;
            }
            return;
        }
        out.push(if self.after_cr { JobEvent::Replace(text) } else { JobEvent::Line(text) });
        self.after_cr = terminator == Some(b'\r');
    }

    fn feed(&mut self, bytes: &[u8]) -> Vec<JobEvent> {
        let mut out = Vec::new();
        for &b in bytes {
            if b == b'\n' || b == b'\r' {
                self.emit(&mut out, Some(b));
            } else {
                self.buf.push(b);
            }
        }
        out
    }

    fn finish(&mut self) -> Vec<JobEvent> {
        let mut out = Vec::new();
        self.emit(&mut out, None);
        out
    }
}

/// Where a job reads and writes. `Dirs::default_dirs()` is the real config.
#[derive(Clone, Debug)]
pub struct Dirs {
    pub registry: PathBuf,
    pub receipts: PathBuf,
}

impl Dirs {
    pub fn default_dirs() -> Self {
        Dirs { registry: registry::apps_dir(), receipts: receipts_dir() }
    }
}

/// Why a job may not start, if it may not.
pub fn check(action: Action, app: &StoreApp, dirs: &Dirs) -> Result<(), String> {
    if app.root {
        return Err("needs root: run it over SSH".into());
    }
    let installed = registry::load(&dirs.registry).entries.iter().any(|e| e.id == app.id());
    match (action, installed) {
        (Action::Install, true) => Err("already installed".into()),
        (Action::Uninstall, false) => Err("not installed".into()),
        _ => Ok(()),
    }
}

/// How long a closed app gets to exit before an uninstall starts anyway.
const EXIT_WAIT: Duration = Duration::from_secs(8);

/// Per-step limits. `timeout` ends a step that never finishes; `grace` is how
/// long the output readers get once `sh` has exited (a background child can
/// hold the pipe open) before the step's process group is killed.
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub timeout: Duration,
    pub grace: Duration,
}

impl Limits {
    pub const DEFAULT: Limits = Limits { timeout: Duration::from_secs(15 * 60), grace: Duration::from_secs(1) };
}

/// True while `pid` exists and is not a zombie. Always false without /proc.
fn pid_alive(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/stat"))
        .ok()
        .and_then(|s| s.rsplit_once(')').map(|(_, rest)| !rest.trim_start().starts_with('Z')))
        .unwrap_or(false)
}

/// Blocks until none of `pids` is alive, or `timeout`. Returns whether they all exited.
pub fn wait_for_exit(id: &str, pids: &[u32], timeout: Duration) -> bool {
    let started = Instant::now();
    let mut announced = false;
    while pids.iter().any(|&p| pid_alive(p)) {
        if !announced {
            eprintln!("[store] waiting for {id} to exit");
            announced = true;
        }
        if started.elapsed() >= timeout {
            eprintln!("[store] {id} still running after {} s, continuing", timeout.as_secs());
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    true
}

/// Starts a job on its own thread. Returns `Err(reason)` at once if it may not
/// run. Otherwise `prepare` runs on the calling thread (storefeed closes the
/// running app there), the worker waits for the PIDs it returns to exit (up to
/// 8 s), then the steps run; events arrive on the worker thread through
/// `on_event` and the last one is `Done` or `Failed`. The caller keeps the
/// "one job at a time" guard.
pub fn run_job(
    action: Action,
    app: StoreApp,
    prepare: impl FnOnce() -> Vec<u32>,
    on_event: impl Fn(JobEvent) + Send + 'static,
) -> Result<(), String> {
    let dirs = Dirs::default_dirs();
    check(action, &app, &dirs)?;
    let wait = prepare();
    let src = source();
    std::thread::spawn(move || {
        let _ = run_job_blocking(&dirs, &src, action, &app, &wait, Limits::DEFAULT, &on_event);
    });
    Ok(())
}

/// The whole job on the current thread (what `run_job` runs, and what the tests call).
pub fn run_job_blocking(
    dirs: &Dirs,
    source: &str,
    action: Action,
    app: &StoreApp,
    wait: &[u32],
    limits: Limits,
    on_event: &dyn Fn(JobEvent),
) -> Result<(), String> {
    let id = app.id();
    let verb = action.as_str();
    if let Err(reason) = check(action, app, dirs) {
        eprintln!("[store] refused {verb} {id}: {reason}");
        return Err(reason);
    }
    wait_for_exit(id, wait, EXIT_WAIT);
    let steps = match action {
        Action::Install => app.install.clone(),
        Action::Uninstall => uninstall_steps(app, read_receipt(&dirs.receipts, id).as_ref()),
    };
    let n = steps.len();
    let fail = |k: usize, code: i32, message: String, last: &str| {
        let at = if n == 0 { "-/0".to_string() } else { format!("{k}/{n}") };
        eprintln!("[store] {verb} {id} FAILED at step {at} (exit {code}): {last}");
        on_event(JobEvent::Failed { k, code, message });
        Err("failed".to_string())
    };
    if n == 0 {
        eprintln!("[store] {verb} {id}: step -/0 (no commands, only the app entry)");
    }
    for (i, step) in steps.iter().enumerate() {
        let k = i + 1;
        eprintln!("[store] {verb} {id}: step {k}/{n}: {step}");
        on_event(JobEvent::Step { k, n, command: step.clone() });
        match run_step(step, limits, on_event) {
            Ok((StepEnd::Exit(0), _)) => {}
            Ok((StepEnd::Exit(code), last)) => {
                let tail = if last.is_empty() { String::new() } else { format!(": {last}") };
                return fail(k, code, format!("Step {k} of {n} failed (exit {code}){tail}"), &last);
            }
            Ok((StepEnd::TimedOut, last)) => {
                let after = limits.timeout.as_secs();
                let after = if after >= 60 && after % 60 == 0 { format!("{} min", after / 60) } else { format!("{after} s") };
                return fail(k, -1, format!("Step {k} of {n} timed out after {after}"), &last);
            }
            Err(e) => return fail(k, -1, format!("Step {k} of {n} could not run: {e}"), &e),
        }
    }
    let last_k = n.max(1);
    let finish = match action {
        Action::Install => {
            let header = format!(
                "Installed by the Lumen store from {source} on {}",
                chrono::Local::now().format("%Y-%m-%d")
            );
            registry::write_entry(&dirs.registry, &app.entry, &header).and_then(|_| {
                write_receipt(&dirs.receipts, app, source).map(|_| ()).inspect_err(|_| {
                    let _ = registry::remove_entry(&dirs.registry, id);
                })
            })
        }
        Action::Uninstall => match registry::remove_entry(&dirs.registry, id) {
            Ok(true) => remove_receipt(&dirs.receipts, id).map(|_| ()),
            Ok(false) => Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("no {id}.toml to remove"))),
            Err(e) => Err(e),
        },
    };
    if let Err(e) = finish {
        let what = if action == Action::Install { "write the app entry" } else { "remove the app entry" };
        return fail(last_k, 0, format!("Steps passed but Lumen could not {what}: {e}"), &e.to_string());
    }
    eprintln!("[store] {verb} {id} ok");
    on_event(JobEvent::Done);
    Ok(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepEnd {
    Exit(i32),
    TimedOut,
}

/// SIGKILLs every process in the step's group (`sh` is its leader).
fn kill_group(pgid: u32) {
    #[cfg(target_os = "linux")]
    unsafe {
        libc::kill(-(pgid as i32), libc::SIGKILL);
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = Command::new("sh")
            .args(["-c", &format!("kill -KILL -- -{pgid}")])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Runs one step; returns how it ended and the last non-empty output line.
/// stdout and stderr share one pipe (so the `\r` replacement logic sees one
/// ordered stream). The step ends when `sh` exits: output still arriving gets
/// `limits.grace`, then the whole process group is killed. A step running
/// longer than `limits.timeout` is killed and reported as `TimedOut`.
fn run_step(step: &str, limits: Limits, on_event: &dyn Fn(JobEvent)) -> Result<(StepEnd, String), String> {
    let (reader, writer) = std::io::pipe().map_err(|e| e.to_string())?;
    let mut cmd = Command::new("sh");
    cmd.args(["-c", step])
        .stdin(Stdio::null())
        .stdout(writer.try_clone().map_err(|e| e.to_string())?)
        .stderr(writer);
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    drop(cmd); // closes the parent's copies of the write end, so EOF can arrive
    let pgid = child.id();

    let (tx, rx) = mpsc::channel::<JobEvent>();
    let handle = std::thread::spawn(move || {
        let mut reader = reader;
        let mut split = LineSplitter::default();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    for ev in split.feed(&chunk[..n]) {
                        let _ = tx.send(ev);
                    }
                }
            }
        }
        for ev in split.finish() {
            let _ = tx.send(ev);
        }
    });

    let mut last = String::new();
    let mut forward = |ev: JobEvent| {
        if let JobEvent::Line(t) | JobEvent::Replace(t) = &ev {
            last = t.clone();
        }
        on_event(ev);
    };
    let started = Instant::now();
    let mut status = None;
    let mut exited_at: Option<Instant> = None;
    let mut timed_out = false;
    let mut eof = false;
    loop {
        if eof {
            std::thread::sleep(Duration::from_millis(50));
        } else {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(ev) => forward(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => eof = true,
            }
        }
        if status.is_none() {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                status = Some(s);
                exited_at = Some(Instant::now());
            } else if started.elapsed() >= limits.timeout {
                kill_group(pgid);
                timed_out = true;
                status = Some(child.wait().map_err(|e| e.to_string())?);
                exited_at = Some(Instant::now());
            }
        }
        if status.is_some() && eof {
            break;
        }
        if exited_at.is_some_and(|t| t.elapsed() >= limits.grace) {
            break;
        }
    }
    if !eof {
        // `sh` is gone but something still holds the pipe: end the group, then
        // take what is left (a process that left the group can keep the pipe, so
        // don't wait for the reader beyond a short drain).
        kill_group(pgid);
        let drain = Instant::now();
        while drain.elapsed() < Duration::from_millis(500) {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(ev) => forward(ev),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    }
    if handle.is_finished() {
        let _ = handle.join();
    }
    let status = status.ok_or_else(|| "step ended without an exit status".to_string())?;
    let end = if timed_out { StepEnd::TimedOut } else { StepEnd::Exit(status.code().unwrap_or(-1)) };
    Ok((end, last))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    const MANIFEST: &str = include_str!("../store/manifest.json");

    fn app_json(extra: &str) -> String {
        format!(
            r#"{{"version":1,"apps":[{{"id":"a","name":"A","launch":"a","description":"d","install":[],"uninstall":[]{extra}}}]}}"#
        )
    }

    #[test]
    fn shipped_manifest_has_no_errors() {
        let m = parse_manifest(MANIFEST);
        assert!(m.errors.is_empty(), "{:?}", m.errors);
        let ids: Vec<_> = m.apps.iter().map(|a| a.id()).collect();
        assert_eq!(ids, ["jellyfin", "steamlink", "kolourpaint", "store-test"]);
        let by = |id: &str| m.apps.iter().find(|a| a.id() == id).unwrap();
        assert!(by("steamlink").root && !by("kolourpaint").root);
        assert!(by("jellyfin").install.is_empty());
        // The manifest's jellyfin/steamlink registry fields match the shipped TOMLs.
        for (name, text) in registry::SHIPPED {
            let e = registry::parse(name, text).unwrap();
            if let Some(a) = m.apps.iter().find(|a| a.id() == e.id) {
                assert_eq!(a.entry, e, "{} differs from config/apps", e.id);
            }
        }
    }

    #[test]
    fn manifest_good_and_bad_apps() {
        let m = parse_manifest(&app_json(""));
        assert!(m.errors.is_empty() && m.apps.len() == 1 && !m.fatal);

        let m = parse_manifest(&app_json(r#","bogus":1"#));
        assert!(m.apps.is_empty() && m.errors[0].contains("bogus"), "{:?}", m.errors);

        let m = parse_manifest(&app_json(r#","install":"flatpak install x""#).replace(r#""install":[],"#, ""));
        assert!(m.apps.is_empty() && m.errors[0].contains("install"), "{:?}", m.errors);

        let m = parse_manifest(&app_json("").replace(r#""uninstall":[]"#, r#""uninstall":[""]"#));
        assert!(m.apps.is_empty() && m.errors[0].contains("non-empty"), "{:?}", m.errors);

        let m = parse_manifest(&app_json("").replace(r#""install":[],"#, ""));
        assert!(m.apps.is_empty() && m.errors[0].contains("missing"), "{:?}", m.errors);

        let m = parse_manifest(&app_json(r#","root":"yes""#));
        assert!(m.apps.is_empty() && m.errors[0].contains("root"));
    }

    #[test]
    fn manifest_fatal_cases() {
        assert!(parse_manifest("nope").fatal);
        let m = parse_manifest(r#"{"version":2,"apps":[]}"#);
        assert!(m.fatal && m.errors[0].contains("version 2"));
        assert!(parse_manifest(r#"{"apps":[]}"#).fatal);
        assert!(parse_manifest(r#"{"version":1}"#).fatal);
    }

    #[test]
    fn manifest_duplicate_ids_and_isolated_errors() {
        let one = r#"{"id":"a","name":"A","launch":"a","install":[],"uninstall":[]}"#;
        let bad = r#"{"id":"b","name":"B","install":[],"uninstall":[]}"#;
        let text = format!(r#"{{"version":1,"apps":[{one},{bad},{one}]}}"#);
        let m = parse_manifest(&text);
        assert_eq!(m.apps.len(), 1);
        assert_eq!(m.errors.len(), 2);
        assert!(m.errors[0].starts_with("b:") && m.errors[1].contains("duplicate"));
        assert!(!m.fatal);
    }

    #[test]
    fn app_value_round_trips() {
        for a in parse_manifest(MANIFEST).apps {
            assert_eq!(parse_app(a.to_value()).unwrap(), a);
        }
    }

    #[test]
    fn splitter_handles_cr_and_lf() {
        let mut s = LineSplitter::default();
        let mut evs = s.feed(b"start\nProgress 10%\rProgress 50%\rProgress 100%\r\ndone\nlast");
        evs.extend(s.feed(b" bit"));
        evs.extend(s.finish());
        assert_eq!(
            evs,
            vec![
                JobEvent::Line("start".into()),
                JobEvent::Line("Progress 10%".into()),
                JobEvent::Replace("Progress 50%".into()),
                JobEvent::Replace("Progress 100%".into()),
                JobEvent::Line("done".into()),
                JobEvent::Line("last bit".into()),
            ]
        );
    }

    #[test]
    fn splitter_joins_chunks() {
        let mut s = LineSplitter::default();
        let mut evs = s.feed(b"hel");
        evs.extend(s.feed(b"lo\nwor"));
        evs.extend(s.feed(b"ld\n"));
        assert_eq!(evs, vec![JobEvent::Line("hello".into()), JobEvent::Line("world".into())]);
    }

    fn temp(tag: &str) -> Dirs {
        let base = std::env::temp_dir().join(format!("lumen-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        Dirs { registry: base.join("apps"), receipts: base.join("store") }
    }

    fn app(id: &str, install: &[&str], uninstall: &[&str]) -> StoreApp {
        let v = json!({
            "id": id, "name": id, "launch": id, "description": "d",
            "install": install, "uninstall": uninstall,
        });
        parse_app(v).unwrap()
    }

    fn run(dirs: &Dirs, action: Action, a: &StoreApp) -> (Result<(), String>, Vec<JobEvent>) {
        let log = Mutex::new(Vec::new());
        let r = run_job_blocking(dirs, "test-source", action, a, &[], Limits::DEFAULT, &|e| log.lock().unwrap().push(e));
        (r, log.into_inner().unwrap())
    }

    fn files(dir: &Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default()
    }

    #[test]
    fn install_success_writes_entry_and_receipt_then_uninstall_removes_them() {
        let dirs = temp("ok");
        let a = app("x", &["true", "echo hi"], &["echo bye"]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_ok());
        assert_eq!(ev.last(), Some(&JobEvent::Done));
        assert!(ev.contains(&JobEvent::Step { k: 2, n: 2, command: "echo hi".into() }));
        assert!(ev.contains(&JobEvent::Line("hi".into())));
        let toml = std::fs::read_to_string(dirs.registry.join("x.toml")).unwrap();
        assert!(toml.starts_with("# Installed by the Lumen store from test-source on "));
        assert_eq!(registry::load(&dirs.registry).entries, vec![a.entry.clone()]);
        assert_eq!(read_receipt(&dirs.receipts, "x").unwrap(), a);
        let raw: Value = serde_json::from_str(&std::fs::read_to_string(dirs.receipts.join("x.json")).unwrap()).unwrap();
        assert_eq!(raw["source"], "test-source");
        assert!(raw["installed_at"].as_str().unwrap().contains('T'));

        // Installed twice is refused.
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert_eq!(r.unwrap_err(), "already installed");
        assert!(ev.is_empty());

        // Uninstall uses the receipt's steps, not the (changed) manifest's.
        let changed = app("x", &["true"], &["exit 9"]);
        let (r, ev) = run(&dirs, Action::Uninstall, &changed);
        assert!(r.is_ok(), "{ev:?}");
        assert!(ev.contains(&JobEvent::Line("bye".into())));
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
        assert_eq!(run(&dirs, Action::Uninstall, &changed).0.unwrap_err(), "not installed");
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn empty_steps_still_write_the_entry() {
        let dirs = temp("empty");
        let a = app("e", &[], &[]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_ok());
        assert_eq!(ev, vec![JobEvent::Done]);
        assert!(dirs.registry.join("e.toml").exists());
        assert!(run(&dirs, Action::Uninstall, &a).0.is_ok());
        assert!(!dirs.registry.join("e.toml").exists());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn failing_step_writes_nothing() {
        let dirs = temp("fail");
        let a = app("f", &["echo a; exit 3"], &[]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_err());
        match ev.last() {
            Some(JobEvent::Failed { k, code, message }) => {
                assert_eq!((*k, *code), (1, 3));
                assert!(message.contains("exit 3") && message.contains(": a"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn second_step_failure_stops_there() {
        let dirs = temp("fail2");
        let a = app("g", &["true", "echo oops >&2; exit 2", "echo never"], &[]);
        let (_, ev) = run(&dirs, Action::Install, &a);
        assert!(matches!(ev.last(), Some(JobEvent::Failed { k: 2, code: 2, .. })), "{ev:?}");
        assert!(!ev.contains(&JobEvent::Line("never".into())));
        assert!(ev.contains(&JobEvent::Line("oops".into())), "stderr is merged");
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn failed_uninstall_removes_nothing() {
        let dirs = temp("failun");
        let a = app("h", &[], &["exit 1"]);
        assert!(run(&dirs, Action::Install, &a).0.is_ok());
        let (r, ev) = run(&dirs, Action::Uninstall, &a);
        assert!(r.is_err() && matches!(ev.last(), Some(JobEvent::Failed { k: 1, code: 1, .. })));
        assert!(dirs.registry.join("h.toml").exists() && dirs.receipts.join("h.json").exists());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn carriage_return_progress_replaces_lines() {
        let dirs = temp("cr");
        let a = app("p", &["printf 'a\\rb\\rc\\nend\\n'"], &[]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_ok());
        let lines: Vec<_> = ev.iter().filter(|e| matches!(e, JobEvent::Line(_) | JobEvent::Replace(_))).cloned().collect();
        assert_eq!(
            lines,
            vec![JobEvent::Line("a".into()), JobEvent::Replace("b".into()), JobEvent::Replace("c".into()), JobEvent::Line("end".into())]
        );
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn root_apps_never_run() {
        let dirs = temp("root");
        let mut a = app("r", &["touch /nonexistent-dir/should-not-run"], &[]);
        a.root = true;
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.unwrap_err().contains("root") && ev.is_empty());
        assert!(files(&dirs.registry).is_empty());
    }

    #[test]
    fn uninstall_steps_prefer_the_receipt() {
        let dirs = temp("steps");
        let a = app("s", &[], &["echo from-receipt"]);
        assert!(run(&dirs, Action::Install, &a).0.is_ok());
        let current = app("s", &[], &["echo from-manifest"]);
        let receipts = read_receipts(&dirs.receipts);
        assert_eq!(uninstall_steps(&current, receipts.get("s")), vec!["echo from-receipt".to_string()]);
        assert_eq!(uninstall_steps(&current, None), vec!["echo from-manifest".to_string()]);
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn receipt_needs_only_the_app() {
        let dirs = temp("minrec");
        std::fs::create_dir_all(&dirs.receipts).unwrap();
        let a = app("m", &[], &["true"]);
        std::fs::write(dirs.receipts.join("m.json"), json!({ "app": a.to_value() }).to_string()).unwrap();
        assert_eq!(read_receipt(&dirs.receipts, "m"), Some(a));
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn background_child_holding_the_pipe_does_not_hang_the_step() {
        let dirs = temp("bg");
        let a = app("b", &["echo started; sleep 30 &"], &[]);
        let t = Instant::now();
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_ok(), "{ev:?}");
        assert!(t.elapsed() < Duration::from_secs(8), "took {:?}", t.elapsed());
        assert!(ev.contains(&JobEvent::Line("started".into())));
        assert_eq!(ev.last(), Some(&JobEvent::Done));
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn a_step_that_never_ends_times_out_as_failed() {
        let dirs = temp("timeout");
        let a = app("t", &["echo hi; sleep 30"], &[]);
        let limits = Limits { timeout: Duration::from_millis(300), grace: Duration::from_millis(200) };
        let log = Mutex::new(Vec::new());
        let t = Instant::now();
        let r = run_job_blocking(&dirs, "s", Action::Install, &a, &[], limits, &|e| log.lock().unwrap().push(e));
        let ev = log.into_inner().unwrap();
        assert!(r.is_err() && t.elapsed() < Duration::from_secs(8), "{:?}", t.elapsed());
        match ev.last() {
            Some(JobEvent::Failed { k: 1, code: -1, message }) => assert!(message.contains("timed out"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn stdout_and_stderr_keep_their_order_in_one_stream() {
        let dirs = temp("merge");
        let a = app("o", &["echo one; echo two >&2; echo three"], &[]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_ok());
        let lines: Vec<_> = ev.iter().filter(|e| matches!(e, JobEvent::Line(_))).cloned().collect();
        assert_eq!(lines, vec![JobEvent::Line("one".into()), JobEvent::Line("two".into()), JobEvent::Line("three".into())]);
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn uninstall_when_the_entry_vanished_during_the_steps_is_failed() {
        let dirs = temp("vanish");
        let a = app("z", &[], &[]);
        assert!(run(&dirs, Action::Install, &a).0.is_ok());
        let rm = format!("rm {}", dirs.registry.join("z.toml").display());
        let gone = app("z", &[], &[&rm]);
        // The receipt's (empty) steps win, so use a changed receipt: rewrite it with the rm step.
        write_receipt(&dirs.receipts, &gone, "test-source").unwrap();
        let (r, ev) = run(&dirs, Action::Uninstall, &gone);
        assert!(r.is_err());
        match ev.last() {
            Some(JobEvent::Failed { code: 0, message, .. }) => assert!(message.contains("no z.toml to remove"), "{message}"),
            other => panic!("{other:?}"),
        }
        assert!(!ev.contains(&JobEvent::Done));
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn zero_step_failure_has_step_one() {
        // The Failed event for a job with no commands still names step 1 (the entry write).
        let dirs = temp("zero");
        let base = dirs.registry.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&dirs.registry, "blocker").unwrap();
        let (r, ev) = run(&dirs, Action::Install, &app("q", &[], &[]));
        assert!(r.is_err());
        assert!(matches!(ev.last(), Some(JobEvent::Failed { k: 1, code: 0, .. })), "{ev:?}");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wait_for_exit_follows_a_real_process() {
        let mut child = Command::new("sleep").arg("0.4").spawn().unwrap();
        let pid = child.id();
        std::thread::spawn(move || {
            let _ = child.wait();
        });
        assert!(wait_for_exit("t", &[pid], Duration::from_secs(5)));
        let mut long = Command::new("sleep").arg("30").spawn().unwrap();
        assert!(!wait_for_exit("t", &[long.id()], Duration::from_millis(300)));
        let _ = long.kill();
        let _ = long.wait();
        assert!(wait_for_exit("t", &[], Duration::from_millis(10)));
    }

    #[test]
    fn fetch_reads_local_files_and_reports_errors() {
        let dirs = temp("fetch");
        std::fs::create_dir_all(&dirs.registry).unwrap();
        let p = dirs.registry.join("m.json");
        std::fs::write(&p, MANIFEST).unwrap();
        let path = p.to_string_lossy().into_owned();
        assert_eq!(fetch(&path).unwrap(), MANIFEST);
        assert_eq!(fetch(&format!("file://{path}")).unwrap(), MANIFEST);
        assert_eq!(load_manifest(&path).unwrap().apps.len(), 4);
        assert!(fetch("/nonexistent/manifest.json").is_err());
        assert!(load_manifest("/nonexistent/manifest.json").is_err());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn non_string_step_is_rejected() {
        let m = parse_manifest(&app_json("").replace(r#""install":[]"#, r#""install":["ok", 5]"#));
        assert!(m.apps.is_empty() && m.errors[0].contains("install"), "{:?}", m.errors);
        let m = parse_manifest(&app_json("").replace(r#""uninstall":[]"#, r#""uninstall":[null]"#));
        assert!(m.apps.is_empty() && m.errors[0].contains("uninstall"), "{:?}", m.errors);
    }

    #[test]
    fn registry_write_failure_after_good_steps_is_failed_and_leaves_nothing() {
        let dirs = temp("regfail");
        let base = dirs.registry.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&base).unwrap();
        // The registry "directory" is a plain file, so write_entry cannot create it.
        std::fs::write(&dirs.registry, "blocker").unwrap();
        let a = app("w", &["echo built"], &[]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_err());
        match ev.last() {
            Some(JobEvent::Failed { k, code, message }) => {
                assert_eq!((*k, *code), (1, 0));
                assert!(message.contains("could not write the app entry"), "{message}");
            }
            other => panic!("{other:?}"),
        }
        assert!(!ev.contains(&JobEvent::Done));
        assert!(files(&dirs.receipts).is_empty());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn receipt_write_failure_rolls_back_the_registry_entry() {
        let dirs = temp("recfail");
        let base = dirs.registry.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&dirs.receipts, "blocker").unwrap();
        let a = app("v", &[], &[]);
        let (r, ev) = run(&dirs, Action::Install, &a);
        assert!(r.is_err());
        assert!(matches!(ev.last(), Some(JobEvent::Failed { code: 0, .. })), "{ev:?}");
        assert!(files(&dirs.registry).is_empty(), "entry must not be left without a receipt");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn unreadable_receipt_falls_back_to_manifest_uninstall_steps() {
        let dirs = temp("badrec");
        let a = app("u", &[], &["echo from-manifest"]);
        assert!(run(&dirs, Action::Install, &a).0.is_ok());
        std::fs::write(dirs.receipts.join("u.json"), "not json").unwrap();
        assert!(read_receipt(&dirs.receipts, "u").is_none());
        let (r, ev) = run(&dirs, Action::Uninstall, &a);
        assert!(r.is_ok(), "{ev:?}");
        assert!(ev.contains(&JobEvent::Line("from-manifest".into())));
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }
}
