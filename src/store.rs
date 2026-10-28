// Store: manifest fetch + parse, install/uninstall runner, receipts. See AGENT_JOBS.md.
//
// Pure logic, no Slint: the UI glue (storefeed.rs) calls `load_manifest`,
// `uninstall_steps` and `run_job`. Steps run as `sh -c '<step>'` as
// the current user. `root: true` apps never run a manifest string: they make
// one call to the root helper (`platform::RootInstaller`, needs the PIN) for
// the packages on the helper's own allowlist. Lumen itself never escalates.
// There is no rollback: a failed job leaves whatever the steps did, but
// writes no registry entry and removes none.

use std::cell::Cell;
use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::platform::{self, RootEvent, RootInstaller, RootOp, RootOutcome, RootSet, RootStatus};
use crate::registry::{self, Entry};

pub(crate) const DEFAULT_URL: &str = "https://raw.githubusercontent.com/Novapool/lumen-tv/main/store/manifest.json";

/// Keys of a manifest app that aren't registry fields.
const STORE_KEYS: [&str; 9] = ["description", "detail", "install", "uninstall", "root", "category", "notes", "arch", "tile"];

/// Registry/launch keys a `tile: false` app must not carry (it has no registry entry).
const TILE_ONLY_KEYS: [&str; 13] = [
    "type", "launch", "url", "fallback_url", "app_id", "process", "background", "exclusive", "pause_on_home", "icon", "user_agent",
    "browser_args", "keys",
];

/// The Store's sections, in display order. A manifest `category` must be one of these.
pub const CATEGORIES: [&str; 4] = ["Media", "Music", "Games", "Utilities"];
/// The section of an app that names no `category`.
pub const DEFAULT_CATEGORY: &str = "Media";

/// One app in the manifest: the registry entry it would install, plus store-only data.
#[derive(Clone, Debug, PartialEq)]
pub struct StoreApp {
    pub entry: Entry,
    pub description: String,
    pub detail: Option<String>,
    pub install: Vec<String>,
    pub uninstall: Vec<String>,
    pub root: bool,
    /// One of `CATEGORIES` (the Store section).
    pub category: String,
    /// Setup notes shown on the details screen (empty = none).
    pub notes: String,
    /// `uname -m` values this app runs on; empty = every architecture.
    pub arch: Vec<String>,
    /// False = a background service with no Home tile: no registry entry is
    /// written, and "installed" means the receipt exists. `entry` then holds
    /// only the id, name and Store-row look.
    pub tile: bool,
}

impl StoreApp {
    /// True when the app is offered on `running` (a `uname -m` value).
    pub fn runs_on(&self, running: &str) -> bool {
        self.arch.is_empty() || self.arch.iter().any(|a| a == running)
    }

    pub fn id(&self) -> &str {
        &self.entry.id
    }

    /// The manifest-format JSON object for this app (also what a receipt stores).
    pub fn to_value(&self) -> Value {
        let mut v = serde_json::to_value(&self.entry).expect("Entry serializes");
        let o = v.as_object_mut().expect("Entry is an object");
        if !self.tile {
            o.insert("tile".into(), json!(false));
            for k in TILE_ONLY_KEYS {
                o.remove(k);
            }
        }
        o.insert("description".into(), json!(self.description));
        if let Some(d) = &self.detail {
            o.insert("detail".into(), json!(d));
        }
        o.insert("install".into(), json!(self.install));
        o.insert("uninstall".into(), json!(self.uninstall));
        if self.root {
            o.insert("root".into(), json!(true));
        }
        o.insert("category".into(), json!(self.category));
        if !self.notes.is_empty() {
            o.insert("notes".into(), json!(self.notes));
        }
        if !self.arch.is_empty() {
            o.insert("arch".into(), json!(self.arch));
        }
        v
    }
}

/// The running machine's `uname -m` (falls back to the compile-time arch).
pub fn running_arch() -> String {
    Command::new("uname")
        .arg("-m")
        .stdin(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| std::env::consts::ARCH.to_string())
}

/// Position of a category in the Store's section order.
pub fn category_rank(category: &str) -> usize {
    CATEGORIES.iter().position(|c| *c == category).unwrap_or(CATEGORIES.len())
}

/// The apps offered on `running`, grouped by category in `CATEGORIES` order
/// (manifest order kept inside a category).
pub fn store_view(apps: &[StoreApp], running: &str) -> Vec<StoreApp> {
    let mut v: Vec<StoreApp> = apps.iter().filter(|a| a.runs_on(running)).cloned().collect();
    v.sort_by_key(|a| category_rank(&a.category));
    v
}

/// Per item of a category-grouped list: (0-based section index, first item of its section).
pub fn section_layout(apps: &[StoreApp]) -> Vec<(usize, bool)> {
    let mut section = 0;
    let mut prev: Option<&str> = None;
    apps.iter()
        .enumerate()
        .map(|(i, a)| {
            let start = prev != Some(a.category.as_str());
            if start && i > 0 {
                section += 1;
            }
            prev = Some(a.category.as_str());
            (section, start)
        })
        .collect()
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
    let category = match o.remove("category") {
        None => DEFAULT_CATEGORY.to_string(),
        Some(Value::String(s)) if CATEGORIES.contains(&s.as_str()) => s,
        Some(_) => return Err(format!("\"category\" should be one of {}", CATEGORIES.join(", "))),
    };
    let notes = match o.remove("notes") {
        None | Some(Value::Null) => String::new(),
        Some(Value::String(s)) => s,
        Some(_) => return Err("\"notes\" should be a string".into()),
    };
    let arch = match o.remove("arch") {
        None => Vec::new(),
        Some(Value::Array(items)) => items
            .into_iter()
            .map(|a| match a {
                Value::String(s) if !s.trim().is_empty() => Ok(s),
                _ => Err("\"arch\" should be a list of non-empty strings (uname -m values)".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?,
        Some(_) => return Err("\"arch\" should be a list of strings (uname -m values)".into()),
    };
    let tile = match o.remove("tile") {
        None => true,
        Some(Value::Bool(b)) => b,
        Some(_) => return Err("\"tile\" should be true or false".into()),
    };
    debug_assert!(STORE_KEYS.iter().all(|k| !o.contains_key(*k)));
    if !tile {
        if let Some(k) = TILE_ONLY_KEYS.iter().find(|k| o.contains_key(**k)) {
            return Err(format!("\"{k}\" can't be used with \"tile\": false (the app has no Home tile)"));
        }
    }
    let entry: Entry = serde_json::from_value(Value::Object(o)).map_err(|e| e.to_string())?;
    if tile {
        entry.validate()?;
    } else {
        entry.validate_identity()?;
    }
    Ok(StoreApp { entry, description, detail, install, uninstall, root, category, notes, arch, tile })
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

/// settings.toml `store_url`, else `LUMEN_STORE_URL`, else the public manifest.
pub fn source() -> String {
    let (s, err) = crate::settings::load();
    if let Some(e) = err {
        eprintln!("[settings] {e}");
    }
    crate::settings::resolve_store_url(&s, std::env::var("LUMEN_STORE_URL").ok().as_deref(), DEFAULT_URL).0
}

/// Blocking: reads a local path / `file://` URL, or runs `curl -fsSL --max-time 20`.
pub fn fetch(source: &str) -> Result<String, String> {
    if !source.contains("://") || source.starts_with("file://") {
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
    /// Root apps only: the helper refused the PIN. Ends the job; nothing was
    /// installed or written. `tries_left` is None once locked.
    PinRejected { tries_left: Option<u32>, locked_for_s: u32 },
    /// Root apps only: the helper has no PIN set. Ends the job.
    NoPin,
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

/// The helper's status for the UI glue (blocking: call on a worker thread).
pub fn root_status() -> RootStatus {
    platform::root_installer().status()
}

const NEEDS_SSH: &str = "needs root: run it over SSH";
const NEEDS_PIN: &str = "needs the PIN";

/// Why a root app can't use the helper, if it can't: no helper, no PIN set, or
/// its id isn't on the helper's allowlist. The caller falls back to the SSH text.
pub fn root_refusal(app: &StoreApp, status: &RootStatus) -> Option<String> {
    if !status.available || !status.pin_set || !status.sets.contains_key(app.id()) {
        return Some(NEEDS_SSH.into());
    }
    None
}

/// True when the app is on this machine: its registry entry exists, or, for a
/// `tile: false` app (no entry), its receipt does.
pub fn is_installed(app: &StoreApp, registry_ids: &HashSet<String>, receipts_dir: &Path) -> bool {
    if app.tile {
        registry_ids.contains(app.id())
    } else {
        receipts_dir.join(format!("{}.json", app.id())).exists()
    }
}

/// Why a job may not start, if it may not. `status` is the helper's status
/// (`RootStatus::default()` = no helper); it is only looked at for root apps.
pub fn check(action: Action, app: &StoreApp, dirs: &Dirs, status: &RootStatus) -> Result<(), String> {
    if app.root {
        if let Some(why) = root_refusal(app, status) {
            return Err(why);
        }
    }
    let ids: HashSet<String> = registry::load(&dirs.registry).entries.into_iter().map(|e| e.id).collect();
    let installed = is_installed(app, &ids, &dirs.receipts);
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
/// `on_event` and the last one is `Done`, `Failed`, `PinRejected` or `NoPin`.
/// The caller keeps the "one job at a time" guard.
///
/// `status` is the helper's status as the caller already fetched it (on a
/// worker thread): this call never touches the socket on the calling thread.
/// The worker asks the helper again before running. `pin` is required for root
/// apps and ignored otherwise; it moves into the worker and is dropped when the
/// job ends.
pub fn run_job(
    action: Action,
    app: StoreApp,
    status: &RootStatus,
    pin: Option<String>,
    prepare: impl FnOnce() -> Vec<u32>,
    on_event: impl Fn(JobEvent) + Send + 'static,
) -> Result<(), String> {
    let dirs = Dirs::default_dirs();
    check(action, &app, &dirs, status)?;
    if app.root && pin.as_deref().is_none_or(str::is_empty) {
        return Err(NEEDS_PIN.into());
    }
    let wait = prepare();
    let src = source();
    std::thread::spawn(move || {
        let helper = platform::root_installer();
        let root = Root { helper: helper.as_ref(), pin: pin.as_deref() };
        let _ = run_job_blocking(&dirs, &src, action, &app, &wait, Limits::DEFAULT, root, &on_event);
    });
    Ok(())
}

/// The helper and the PIN for a root job (the PIN is unused by user-level apps).
#[derive(Clone, Copy)]
pub struct Root<'a> {
    pub helper: &'a dyn RootInstaller,
    pub pin: Option<&'a str>,
}

/// The whole job on the current thread (what `run_job` runs, and what the tests call).
#[allow(clippy::too_many_arguments)]
pub fn run_job_blocking(
    dirs: &Dirs,
    source: &str,
    action: Action,
    app: &StoreApp,
    wait: &[u32],
    limits: Limits,
    root: Root,
    on_event: &dyn Fn(JobEvent),
) -> Result<(), String> {
    let id = app.id();
    let verb = action.as_str();
    // Only root apps talk to the helper; everyone else never opens the socket.
    let status = if app.root { root.helper.status() } else { RootStatus::default() };
    if let Err(reason) = check(action, app, dirs, &status) {
        eprintln!("[store] refused {verb} {id}: {reason}");
        return Err(reason);
    }
    if app.root {
        return run_root(dirs, source, action, app, wait, root, &status, on_event);
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
    if let Err(e) = commit(dirs, source, action, app) {
        return fail(n.max(1), 0, commit_error(action, &e), &e.to_string());
    }
    eprintln!("[store] {verb} {id} ok");
    on_event(JobEvent::Done);
    Ok(())
}

/// A root app's job: exactly one helper call, no shell. What it installs comes
/// from the helper's allowlist (`status`), never from the manifest; the
/// `Step` event only describes that call for the progress view.
#[allow(clippy::too_many_arguments)]
fn run_root(
    dirs: &Dirs,
    source: &str,
    action: Action,
    app: &StoreApp,
    wait: &[u32],
    root: Root,
    status: &RootStatus,
    on_event: &dyn Fn(JobEvent),
) -> Result<(), String> {
    let id = app.id();
    let verb = action.as_str();
    let Some(pin) = root.pin.filter(|p| !p.is_empty()) else {
        eprintln!("[store] refused {verb} {id}: no PIN given");
        return Err(NEEDS_PIN.into());
    };
    let set = status.sets.get(id).cloned().unwrap_or_default(); // check() ensured it exists
    wait_for_exit(id, wait, EXIT_WAIT);
    let command = root_command(action, id, &set);
    // The Step (and its log line) is announced only once the helper has accepted the PIN:
    // before the first helper output, or after the call for outcomes other than a PIN refusal.
    let announced = Cell::new(false);
    let announce = || {
        if !announced.replace(true) {
            eprintln!("[store] {verb} {id}: {command}");
            on_event(JobEvent::Step { k: 1, n: 1, command: command.clone() });
        }
    };
    let fail = |code: i32, message: String, last: &str| {
        eprintln!("[store] {verb} {id} FAILED (exit {code}): {last}");
        on_event(JobEvent::Failed { k: 1, code, message });
        Err("failed".to_string())
    };
    let reject = |tries_left: Option<u32>, locked_for_s: u32| {
        eprintln!("[store] {verb} {id}: PIN rejected (locked for {locked_for_s} s)");
        on_event(JobEvent::PinRejected { tries_left, locked_for_s });
        Err("pin rejected".to_string())
    };
    let op = if action == Action::Install { RootOp::Install } else { RootOp::Remove };
    // The helper's own steps (apt-get update, a simulation, the real run) show as lines.
    let outcome = root.helper.run(op, id, pin, &|ev| {
        announce();
        match ev {
            RootEvent::Line(t) | RootEvent::Step(t) => on_event(JobEvent::Line(t)),
            RootEvent::Replace(t) => on_event(JobEvent::Replace(t)),
        }
    });
    if !matches!(outcome, RootOutcome::BadPin { .. } | RootOutcome::Locked { .. } | RootOutcome::NoPin) {
        announce();
    }
    match outcome {
        RootOutcome::Done => {}
        RootOutcome::BadPin { tries_left, locked_for_s } => return reject(tries_left, locked_for_s),
        RootOutcome::Locked { locked_for_s } => return reject(None, locked_for_s),
        RootOutcome::NoPin => {
            eprintln!("[store] {verb} {id}: helper has no PIN");
            on_event(JobEvent::NoPin);
            return Err("no pin".into());
        }
        RootOutcome::Failed { code, message } => {
            let tail = if message.is_empty() { String::new() } else { format!(": {message}") };
            return fail(code, format!("Step 1 of 1 failed (exit {code}){tail}"), &message);
        }
        RootOutcome::NotAllowed(why) => return fail(-1, format!("Step 1 of 1 could not run: the helper refused it: {why}"), &why),
        RootOutcome::Busy => return fail(-1, "Step 1 of 1 could not run: the helper is busy with another job".into(), "busy"),
        RootOutcome::Error(why) => return fail(-1, format!("Step 1 of 1 could not run: {why}"), &why),
        RootOutcome::Unavailable => return fail(-1, "Step 1 of 1 could not run: the helper is not available".into(), "unavailable"),
    }
    if let Err(e) = commit(dirs, source, action, app) {
        return fail(0, commit_error(action, &e), &e.to_string());
    }
    eprintln!("[store] {verb} {id} ok");
    on_event(JobEvent::Done);
    Ok(())
}

/// After the steps passed: write (install) or remove (uninstall) the registry
/// entry and the receipt. Install leaves neither behind if the second write fails.
fn commit(dirs: &Dirs, source: &str, action: Action, app: &StoreApp) -> std::io::Result<()> {
    let id = app.id();
    if !app.tile {
        return match action {
            Action::Install => write_receipt(&dirs.receipts, app, source).map(|_| ()),
            Action::Uninstall => match remove_receipt(&dirs.receipts, id) {
                Ok(true) => Ok(()),
                Ok(false) => Err(std::io::Error::new(std::io::ErrorKind::NotFound, format!("no {id}.json to remove"))),
                Err(e) => Err(e),
            },
        };
    }
    match action {
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
    }
}

fn commit_error(action: Action, e: &std::io::Error) -> String {
    let what = if action == Action::Install { "write the app entry" } else { "remove the app entry" };
    format!("Steps passed but Lumen could not {what}: {e}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepEnd {
    Exit(i32),
    TimedOut,
}

/// The readable description of a root job's one step, from the helper's
/// allowlist set (e.g. `lumen-helper: apt-get install -- sl`).
pub fn root_command(action: Action, id: &str, set: &RootSet) -> String {
    let (verb, pkgs) = match action {
        Action::Install => ("install", &set.install),
        Action::Uninstall => ("remove", &set.remove),
    };
    if pkgs.is_empty() {
        format!("lumen-helper: nothing to {verb} for {id}")
    } else {
        format!("lumen-helper: apt-get {verb} -- {}", pkgs.join(" "))
    }
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
    crate::apps::drop_ambient_caps(&mut cmd);
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

    #[test]
    fn source_precedence_settings_env_default() {
        use crate::settings::{resolve_store_url, Origin, Settings};
        let set = Settings { mode: None, store_url: Some("https://s/m.json".into()) };
        assert_eq!(resolve_store_url(&set, Some("https://e"), DEFAULT_URL), ("https://s/m.json".into(), Origin::Settings));
        let none = Settings::default();
        assert_eq!(resolve_store_url(&none, Some("https://e"), DEFAULT_URL).1, Origin::Env);
        assert_eq!(resolve_store_url(&none, None, DEFAULT_URL), (DEFAULT_URL.into(), Origin::Default));
    }

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
        // Test entries live in store/fixtures, never in the public manifest.
        for t in ["kolourpaint", "store-test", "root-test"] {
            assert!(!ids.contains(&t), "{t} must not ship in the public manifest");
        }
        for d in ["jellyfin", "youtube", "airplay", "steamlink", "kodi"] {
            assert!(ids.contains(&d), "{d} missing");
        }
        let by = |id: &str| m.apps.iter().find(|a| a.id() == id).unwrap();
        assert!(by("steamlink").root && by("kodi").root);
        assert!(by("jellyfin").install.is_empty());
        // The manifest's shipped ids' registry fields match the shipped TOMLs.
        for (name, text) in registry::SHIPPED {
            let e = registry::parse(name, text).unwrap();
            if let Some(a) = m.apps.iter().find(|a| a.id() == e.id) {
                assert_eq!(a.entry, e, "{} differs from config/apps", e.id);
            }
        }
    }

    #[test]
    fn category_defaults_validates_and_orders() {
        let a = parse_manifest(&app_json("")).apps.remove(0);
        assert_eq!(a.category, DEFAULT_CATEGORY);
        assert!(CATEGORIES.contains(&DEFAULT_CATEGORY));
        for c in CATEGORIES {
            let m = parse_manifest(&app_json(&format!(r#","category":"{c}""#)));
            assert!(m.errors.is_empty(), "{c}: {:?}", m.errors);
        }
        for bad in [r#","category":"Nope""#, r#","category":"media""#, r#","category":3"#] {
            let m = parse_manifest(&app_json(bad));
            assert!(m.apps.is_empty() && m.errors[0].contains("category"), "{bad}: {:?}", m.errors);
        }
        let mk = |id: &str, c: &str| parse_app(json!({"id": id, "name": id, "launch": id, "install": [], "uninstall": [], "category": c})).unwrap();
        let apps = [mk("g1", "Games"), mk("u1", "Utilities"), mk("m1", "Media"), mk("g2", "Games"), mk("s1", "Music")];
        let ids: Vec<_> = store_view(&apps, "aarch64").iter().map(|a| a.id().to_string()).collect();
        assert_eq!(ids, ["m1", "s1", "g1", "g2", "u1"]);
    }

    #[test]
    fn arch_hides_entries_not_listed_for_the_running_machine() {
        let mk = |id: &str, arch: &str| parse_app(json!({"id": id, "name": id, "launch": id, "install": [], "uninstall": [], "arch": arch.split_whitespace().collect::<Vec<_>>()})).unwrap();
        let apps = [mk("any", ""), mk("arm", "aarch64"), mk("x86", "x86_64"), mk("both", "aarch64 x86_64")];
        let ids = |m: &str| store_view(&apps, m).iter().map(|a| a.id().to_string()).collect::<Vec<_>>();
        assert_eq!(ids("aarch64"), ["any", "arm", "both"]);
        assert_eq!(ids("x86_64"), ["any", "x86", "both"]);
        assert_eq!(ids("riscv64"), ["any"]);
        for bad in [r#","arch":"aarch64""#, r#","arch":[""]"#, r#","arch":[1]"#] {
            let m = parse_manifest(&app_json(bad));
            assert!(m.apps.is_empty() && m.errors[0].contains("arch"), "{bad}: {:?}", m.errors);
        }
        assert!(!running_arch().is_empty());
    }

    #[test]
    fn category_notes_arch_and_pause_on_home_round_trip_through_receipts() {
        let a = parse_manifest(&app_json(
            r#","category":"Music","notes":"Sign in once.\nThen play.","arch":["aarch64"],"background":"keep-alive","pause_on_home":false"#,
        ))
        .apps
        .remove(0);
        assert_eq!((a.category.as_str(), a.notes.as_str()), ("Music", "Sign in once.\nThen play."));
        assert_eq!(a.arch, ["aarch64"]);
        assert!(!a.entry.pause_on_home);
        assert_eq!(parse_app(a.to_value()).unwrap(), a);
        let dir = std::env::temp_dir().join(format!("lumen-store-newkeys-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        write_receipt(&dir, &a, "src").unwrap();
        assert_eq!(read_receipt(&dir, "a").unwrap(), a);
        // Absent keys stay absent in the serialised form (category is always written).
        let plain = parse_manifest(&app_json("")).apps.remove(0).to_value();
        assert!(plain.get("notes").is_none() && plain.get("arch").is_none());
        assert_eq!(plain["category"], DEFAULT_CATEGORY);
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

    #[test]
    fn section_layout_flags_each_section_start() {
        let mk = |id: &str, cat: &str| {
            let mut a = app(id, &[], &[]);
            a.category = cat.to_string();
            a
        };
        let apps = vec![mk("a", CATEGORIES[0]), mk("b", CATEGORIES[0]), mk("c", CATEGORIES[1])];
        assert_eq!(section_layout(&apps), vec![(0, true), (0, false), (1, true)]);
        assert_eq!(section_layout(&apps[..2]), vec![(0, true), (0, false)]);
        assert!(section_layout(&[]).is_empty());
    }

    fn app(id: &str, install: &[&str], uninstall: &[&str]) -> StoreApp {
        let v = json!({
            "id": id, "name": id, "launch": id, "description": "d",
            "install": install, "uninstall": uninstall,
        });
        parse_app(v).unwrap()
    }

    /// A scripted helper: records every call, answers `status` and `run` as told.
    struct FakeHelper {
        status: RootStatus,
        outcome: RootOutcome,
        stream: Vec<RootEvent>,
        calls: Mutex<Vec<(RootOp, String, String)>>,
    }

    impl FakeHelper {
        fn new(status: RootStatus, outcome: RootOutcome) -> Self {
            FakeHelper { status, outcome, stream: vec![], calls: Mutex::new(vec![]) }
        }
        fn calls(&self) -> Vec<(RootOp, String, String)> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl RootInstaller for FakeHelper {
        fn status(&self) -> RootStatus {
            self.status.clone()
        }
        fn run(&self, op: RootOp, id: &str, pin: &str, on_event: &dyn Fn(RootEvent)) -> RootOutcome {
            self.calls.lock().unwrap().push((op, id.into(), pin.into()));
            for e in &self.stream {
                on_event(e.clone());
            }
            self.outcome.clone()
        }
    }

    fn ready(id: &str, install: &[&str], remove: &[&str]) -> RootStatus {
        let set = platform::RootSet {
            install: install.iter().map(|s| s.to_string()).collect(),
            remove: remove.iter().map(|s| s.to_string()).collect(),
        };
        RootStatus { available: true, pin_set: true, locked_for_s: 0, sets: [(id.to_string(), set)].into(), note: None }
    }

    fn run(dirs: &Dirs, action: Action, a: &StoreApp) -> (Result<(), String>, Vec<JobEvent>) {
        let helper = FakeHelper::new(RootStatus::default(), RootOutcome::Unavailable);
        run_with(dirs, action, a, &helper, None)
    }

    fn run_with(
        dirs: &Dirs,
        action: Action,
        a: &StoreApp,
        helper: &FakeHelper,
        pin: Option<&str>,
    ) -> (Result<(), String>, Vec<JobEvent>) {
        let log = Mutex::new(Vec::new());
        let r = run_job_blocking(dirs, "test-source", action, a, &[], Limits::DEFAULT, Root { helper, pin }, &|e| log.lock().unwrap().push(e));
        (r, log.into_inner().unwrap())
    }

    fn root_app(id: &str) -> StoreApp {
        let mut a = app(id, &["touch /nonexistent-dir/should-not-run"], &["touch /nonexistent-dir/nor-this"]);
        a.root = true;
        a
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

    // Replaces `root_apps_never_run`: root apps now run only through the helper.
    #[test]
    fn root_app_refused_when_helper_unavailable_no_pin_or_not_allowlisted() {
        let dirs = temp("rootrefuse");
        let a = root_app("r");
        let cases = [
            ("unavailable", RootStatus::default()),
            ("no pin", RootStatus { pin_set: false, ..ready("r", &["sl"], &["sl"]) }),
            ("not allowlisted", ready("other", &["sl"], &["sl"])),
        ];
        for (name, status) in cases {
            let helper = FakeHelper::new(status, RootOutcome::Done);
            let (r, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("1234"));
            assert!(r.unwrap_err().contains("run it over SSH"), "{name}");
            assert!(ev.is_empty() && helper.calls().is_empty(), "{name}");
            assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty(), "{name}");
        }
    }

    #[test]
    fn root_check_is_pure_and_lists_the_rules() {
        let dirs = temp("rootcheck");
        let a = root_app("r");
        assert!(check(Action::Install, &a, &dirs, &RootStatus::default()).is_err());
        assert!(check(Action::Install, &a, &dirs, &ready("r", &["sl"], &[])).is_ok());
        assert_eq!(root_refusal(&a, &RootStatus::default()).as_deref(), Some("needs root: run it over SSH"));
        // Locked is not a refusal: the pad shows the countdown on submit.
        let locked = RootStatus { locked_for_s: 30, ..ready("r", &["sl"], &[]) };
        assert!(root_refusal(&a, &locked).is_none());
    }

    #[test]
    fn root_app_needs_a_pin() {
        let dirs = temp("rootnopin");
        let helper = FakeHelper::new(ready("r", &["sl"], &["sl"]), RootOutcome::Done);
        for pin in [None, Some("")] {
            let (r, ev) = run_with(&dirs, Action::Install, &root_app("r"), &helper, pin);
            assert_eq!(r.unwrap_err(), "needs the PIN");
            assert!(ev.is_empty());
        }
        assert!(helper.calls().is_empty() && files(&dirs.registry).is_empty());
    }

    #[test]
    fn root_install_done_writes_entry_and_receipt_with_one_helper_step() {
        let dirs = temp("rootok");
        let a = root_app("r");
        let mut helper = FakeHelper::new(ready("r", &["sl", "extra"], &["sl"]), RootOutcome::Done);
        helper.stream = vec![RootEvent::Step("apt-get update".into()), RootEvent::Line("Setting up sl".into()), RootEvent::Replace("90%".into())];
        let (r, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("4821"));
        assert!(r.is_ok(), "{ev:?}");
        assert_eq!(
            ev,
            vec![
                JobEvent::Step { k: 1, n: 1, command: "lumen-helper: apt-get install -- sl extra".into() },
                JobEvent::Line("apt-get update".into()),
                JobEvent::Line("Setting up sl".into()),
                JobEvent::Replace("90%".into()),
                JobEvent::Done,
            ]
        );
        assert_eq!(helper.calls(), vec![(RootOp::Install, "r".to_string(), "4821".to_string())]);
        assert_eq!(registry::load(&dirs.registry).entries, vec![a.entry.clone()]);
        assert_eq!(read_receipt(&dirs.receipts, "r").unwrap(), a);

        // Uninstall: one helper step from the allowlist's `remove`, then entry + receipt go.
        let (r, ev) = run_with(&dirs, Action::Uninstall, &a, &helper, Some("4821"));
        assert!(r.is_ok(), "{ev:?}");
        assert!(ev.contains(&JobEvent::Step { k: 1, n: 1, command: "lumen-helper: apt-get remove -- sl".into() }));
        assert_eq!(helper.calls()[1], (RootOp::Remove, "r".to_string(), "4821".to_string()));
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn root_pin_rejection_writes_nothing() {
        let dirs = temp("rootbad");
        let a = root_app("r");
        let st = ready("r", &["sl"], &["sl"]);
        let helper = FakeHelper::new(st.clone(), RootOutcome::BadPin { tries_left: Some(3), locked_for_s: 0 });
        let (r, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("0000"));
        assert!(r.is_err());
        assert_eq!(ev.last(), Some(&JobEvent::PinRejected { tries_left: Some(3), locked_for_s: 0 }));
        assert!(!ev.contains(&JobEvent::Done));
        assert!(!ev.iter().any(|e| matches!(e, JobEvent::Step { .. })), "{ev:?}");
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());

        let helper = FakeHelper::new(st.clone(), RootOutcome::Locked { locked_for_s: 55 });
        let (_, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("0000"));
        assert_eq!(ev.last(), Some(&JobEvent::PinRejected { tries_left: None, locked_for_s: 55 }));
        assert!(!ev.iter().any(|e| matches!(e, JobEvent::Step { .. })), "{ev:?}");

        let helper = FakeHelper::new(st, RootOutcome::NoPin);
        let (_, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("0000"));
        assert_eq!(ev.last(), Some(&JobEvent::NoPin));
        assert!(!ev.iter().any(|e| matches!(e, JobEvent::Step { .. })), "{ev:?}");
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
    }

    #[test]
    fn root_pin_rejection_on_uninstall_keeps_the_entry() {
        let dirs = temp("rootbadun");
        let a = root_app("r");
        let st = ready("r", &["sl"], &["sl"]);
        let ok = FakeHelper::new(st.clone(), RootOutcome::Done);
        assert!(run_with(&dirs, Action::Install, &a, &ok, Some("1234")).0.is_ok());
        let bad = FakeHelper::new(st, RootOutcome::BadPin { tries_left: Some(2), locked_for_s: 0 });
        assert!(run_with(&dirs, Action::Uninstall, &a, &bad, Some("0000")).0.is_err());
        assert!(dirs.registry.join("r.toml").exists() && dirs.receipts.join("r.json").exists());
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn root_failures_write_nothing() {
        let dirs = temp("rootfail");
        let a = root_app("r");
        let st = ready("r", &["sl"], &["sl"]);
        let helper = FakeHelper::new(st.clone(), RootOutcome::Failed { code: 100, message: "E: no such package".into() });
        let (r, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("1234"));
        assert!(r.is_err());
        match ev.last() {
            Some(JobEvent::Failed { k: 1, code: 100, message }) => assert!(message.contains("E: no such package"), "{message}"),
            other => panic!("{other:?}"),
        }
        for outcome in [RootOutcome::Busy, RootOutcome::NotAllowed("x".into()), RootOutcome::Error("e".into()), RootOutcome::Unavailable] {
            let helper = FakeHelper::new(st.clone(), outcome);
            let (_, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("1234"));
            assert!(matches!(ev.last(), Some(JobEvent::Failed { k: 1, code: -1, .. })), "{ev:?}");
            assert!(matches!(ev.first(), Some(JobEvent::Step { k: 1, n: 1, .. })), "{ev:?}");
        }
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
    }

    #[test]
    fn hand_installed_root_app_without_a_receipt_uninstalls_through_the_helper() {
        // pitv's existing kodi.toml: a registry entry, no receipt.
        let dirs = temp("rootbyhand");
        let a = root_app("kodi");
        registry::write_entry(&dirs.registry, &a.entry, "by hand").unwrap();
        assert!(files(&dirs.receipts).is_empty());
        // The manifest's SSH strings must not run; the helper's `remove` set is used.
        let helper = FakeHelper::new(ready("kodi", &["kodi", "libretro-mgba"], &["kodi"]), RootOutcome::Done);
        let (r, ev) = run_with(&dirs, Action::Uninstall, &a, &helper, Some("1234"));
        assert!(r.is_ok(), "{ev:?}");
        assert_eq!(helper.calls(), vec![(RootOp::Remove, "kodi".to_string(), "1234".to_string())]);
        assert!(ev.contains(&JobEvent::Step { k: 1, n: 1, command: "lumen-helper: apt-get remove -- kodi".into() }));
        assert_eq!(ev.last(), Some(&JobEvent::Done));
        assert!(files(&dirs.registry).is_empty(), "the TOML is removed");
        assert_eq!(run_with(&dirs, Action::Uninstall, &a, &helper, Some("1234")).0.unwrap_err(), "not installed");
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn root_command_text() {
        let set = platform::RootSet { install: vec!["x".into(), "y".into()], remove: vec![] };
        assert_eq!(root_command(Action::Install, "a", &set), "lumen-helper: apt-get install -- x y");
        assert_eq!(root_command(Action::Uninstall, "a", &set), "lumen-helper: nothing to remove for a");
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
        let helper = FakeHelper::new(RootStatus::default(), RootOutcome::Unavailable);
        let r = run_job_blocking(&dirs, "s", Action::Install, &a, &[], limits, Root { helper: &helper, pin: None }, &|e| log.lock().unwrap().push(e));
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
        assert_eq!(load_manifest(&path).unwrap().apps.len(), parse_manifest(MANIFEST).apps.len());
        assert!(fetch("/nonexistent/manifest.json").is_err());
        // A bare relative path (no scheme) is a local file too, not a curl URL.
        let rel = std::env::current_dir().unwrap().join("Cargo.toml");
        let cwd_rel = fetch("Cargo.toml").unwrap();
        assert_eq!(cwd_rel, std::fs::read_to_string(rel).unwrap());
        assert!(fetch("no/such/manifest.json").unwrap_err().contains("can't read"));
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

    #[test]
    fn the_pin_never_appears_in_job_events() {
        let st = ready("r", &["sl"], &["sl"]);
        let outcomes = [
            RootOutcome::Done,
            RootOutcome::BadPin { tries_left: Some(1), locked_for_s: 0 },
            RootOutcome::Locked { locked_for_s: 9 },
            RootOutcome::NoPin,
            RootOutcome::Failed { code: 100, message: "E: boom".into() },
            RootOutcome::NotAllowed("nope".into()),
            RootOutcome::Busy,
            RootOutcome::Error("e".into()),
            RootOutcome::Unavailable,
        ];
        for (i, outcome) in outcomes.into_iter().enumerate() {
            let dirs = temp(&format!("pinleak{i}"));
            let helper = FakeHelper::new(st.clone(), outcome);
            let (_, ev) = run_with(&dirs, Action::Install, &root_app("r"), &helper, Some("73519"));
            assert!(!format!("{ev:?}").contains("73519"), "{ev:?}");
            let on_disk: String = files(&dirs.registry).into_iter().chain(files(&dirs.receipts)).collect();
            assert!(!on_disk.contains("73519"));
            let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
        }
    }

    #[test]
    fn root_job_never_runs_the_manifests_shell_steps_or_names_its_packages() {
        // The manifest's steps would create this file if run; the helper's set is what is shown and used.
        let dirs = temp("rootnoshell");
        let marker = dirs.registry.parent().unwrap().join("ran");
        let mut a = app("r", &[&format!("mkdir -p {0} && touch {0}/m", marker.display())], &[&format!("touch {}", marker.display())]);
        a.root = true;
        let helper = FakeHelper::new(ready("r", &["from-helper"], &["from-helper"]), RootOutcome::Done);
        let (r, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("1234"));
        assert!(r.is_ok(), "{ev:?}");
        let (r, ev2) = run_with(&dirs, Action::Uninstall, &a, &helper, Some("1234"));
        assert!(r.is_ok(), "{ev2:?}");
        assert!(!marker.exists());
        for e in ev.iter().chain(&ev2) {
            if let JobEvent::Step { n, command, .. } = e {
                assert_eq!(*n, 1);
                assert!(command.contains("from-helper") && !command.contains("touch"), "{command}");
            }
        }
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn root_job_with_an_id_dropped_from_the_status_after_check_still_calls_the_helper_not_a_shell() {
        // run_root looks the set up with unwrap_or_default, but the helper stays the gate.
        let dirs = temp("rootdrop");
        let helper = FakeHelper::new(RootStatus::default(), RootOutcome::NotAllowed("app id is not on the allowlist".into()));
        let r = run_job_blocking(&dirs, "s", Action::Install, &root_app("r"), &[], Limits::DEFAULT, Root { helper: &helper, pin: Some("1234") }, &|_| {});
        assert!(r.is_err());
        assert!(files(&dirs.registry).is_empty() && files(&dirs.receipts).is_empty());
    }

    #[test]
    fn root_job_refuses_an_install_of_an_already_installed_app_without_asking_the_helper() {
        let dirs = temp("rootdup");
        let a = root_app("r");
        let helper = FakeHelper::new(ready("r", &["sl"], &["sl"]), RootOutcome::Done);
        assert!(run_with(&dirs, Action::Install, &a, &helper, Some("1234")).0.is_ok());
        let (r, ev) = run_with(&dirs, Action::Install, &a, &helper, Some("1234"));
        assert_eq!(r.unwrap_err(), "already installed");
        assert!(ev.is_empty());
        assert_eq!(helper.calls().len(), 1, "a refused job must not burn a PIN guess");
        let _ = std::fs::remove_dir_all(dirs.registry.parent().unwrap());
    }

    #[test]
    fn root_commit_failure_after_the_helper_succeeded_is_reported_and_keeps_nothing() {
        // apt changed the system but Lumen could not write its entry: failed, no half state.
        let dirs = temp("rootcommit");
        let base = dirs.registry.parent().unwrap().to_path_buf();
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(&dirs.registry, "a file where the apps dir should be").unwrap();
        let helper = FakeHelper::new(ready("r", &["sl"], &["sl"]), RootOutcome::Done);
        let (r, ev) = run_with(&dirs, Action::Install, &root_app("r"), &helper, Some("1234"));
        assert!(r.is_err());
        assert!(matches!(ev.last(), Some(JobEvent::Failed { k: 1, code: 0, message }) if message.contains("could not write the app entry")), "{ev:?}");
        assert!(!dirs.receipts.join("r.json").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn root_refusal_needs_all_three_conditions() {
        let a = root_app("r");
        let good = ready("r", &["sl"], &["sl"]);
        assert!(root_refusal(&a, &good).is_none());
        for bad in [
            RootStatus { available: false, ..good.clone() },
            RootStatus { pin_set: false, ..good.clone() },
            RootStatus { sets: Default::default(), ..good.clone() },
        ] {
            assert!(root_refusal(&a, &bad).is_some());
        }
    }

    #[test]
    fn shipped_manifest_root_apps_and_helper_allowlist_name_the_same_ids() {
        // A root app missing from the allowlist would only ever show the SSH text; an allowlist
        // set with no root manifest app is root-installable power nothing in the Store offers.
        let m = parse_manifest(MANIFEST);
        let mut root_ids: Vec<_> = m.apps.iter().filter(|a| a.root).map(|a| a.id().to_string()).collect();
        root_ids.sort();
        let al: toml::Table = include_str!("../session/helper-allowlist.toml").parse().unwrap();
        let mut set_ids: Vec<_> = al.keys().cloned().collect();
        set_ids.sort();
        assert_eq!(root_ids, set_ids);
    }

    #[test]
    fn test_fixture_manifest_parses_and_holds_the_four_test_entries() {
        let m = parse_manifest(include_str!("../store/fixtures/test-manifest.json"));
        assert!(!m.fatal && m.errors.is_empty(), "{:?}", m.errors);
        let ids: Vec<_> = m.apps.iter().map(|a| a.id()).collect();
        assert_eq!(ids, ["kolourpaint", "store-test", "root-test", "notile-test"]);
        let by = |id: &str| m.apps.iter().find(|a| a.id() == id).unwrap();
        assert!(by("root-test").root && !by("kolourpaint").root && !by("store-test").root);
        assert!(!by("notile-test").tile && by("kolourpaint").tile);
    }

    // ---- tile: false ----

    fn notile(id: &str) -> StoreApp {
        parse_app(json!({
            "id": id, "name": id, "tile": false, "glyph": "SC", "colors": ["#112233", "#445566"], "order": 24,
            "description": "d", "install": ["true"], "uninstall": ["true"],
        }))
        .unwrap()
    }

    #[test]
    fn tile_false_parses_without_launch_and_tile_defaults_to_true() {
        let a = notile("bg");
        assert!(!a.tile && a.entry.launch.is_none() && a.entry.url.is_none());
        assert!(app("x", &[], &[]).tile);
        let err = parse_app(json!({"id": "a", "name": "a", "tile": "no", "install": [], "uninstall": []})).unwrap_err();
        assert!(err.contains("tile"), "{err}");
    }

    #[test]
    fn tile_false_rejects_every_tile_only_key_by_name() {
        let samples = [
            ("type", json!("command")),
            ("launch", json!("x")),
            ("url", json!("https://x")),
            ("fallback_url", json!("https://x")),
            ("app_id", json!("x")),
            ("process", json!("x")),
            ("background", json!("keep-alive")),
            ("exclusive", json!(true)),
            ("pause_on_home", json!(false)),
            ("icon", json!("x")),
            ("user_agent", json!("x")),
            ("browser_args", json!(["--x"])),
            ("keys", json!({"back": "KEY_ESC"})),
        ];
        assert_eq!(samples.len(), TILE_ONLY_KEYS.len());
        for (k, v) in samples {
            let mut o = json!({"id": "a", "name": "a", "tile": false, "install": [], "uninstall": []});
            o[k] = v;
            let err = parse_app(o).unwrap_err();
            assert!(err.contains(&format!("\"{k}\"")), "{k}: {err}");
        }
    }

    #[test]
    fn tile_false_still_validates_id_name_glyph_and_colors() {
        let bad = |k: &str, v: Value| {
            let mut o = json!({"id": "a", "name": "a", "tile": false, "install": [], "uninstall": []});
            o[k] = v;
            parse_app(o).is_err()
        };
        assert!(bad("id", json!("bad id")));
        assert!(bad("name", json!("  ")));
        assert!(bad("glyph", json!("abc")));
        assert!(bad("colors", json!(["#112233"])));
        assert!(bad("colors", json!(["#112233", "red"])));
    }

    #[test]
    fn tile_false_round_trips_through_to_value() {
        let a = notile("bg");
        let v = a.to_value();
        assert_eq!(v["tile"], json!(false));
        for k in TILE_ONLY_KEYS {
            assert!(v.get(k).is_none(), "{k} leaked into {v}");
        }
        assert_eq!(parse_app(v).unwrap(), a);
        assert!(app("x", &[], &[]).to_value().get("tile").is_none());
    }

    #[test]
    fn tile_false_install_writes_only_the_receipt_and_uninstall_removes_it() {
        let dirs = temp("notile");
        let a = notile("bg");
        let ids = HashSet::new();
        assert!(!is_installed(&a, &ids, &dirs.receipts));
        assert_eq!(run(&dirs, Action::Uninstall, &a).0.unwrap_err(), "not installed");
        run(&dirs, Action::Install, &a).0.unwrap();
        assert!(dirs.receipts.join("bg.json").exists());
        assert!(!dirs.registry.join("bg.toml").exists());
        assert!(!dirs.registry.exists() || std::fs::read_dir(&dirs.registry).unwrap().next().is_none());
        assert!(is_installed(&a, &ids, &dirs.receipts));
        assert_eq!(read_receipt(&dirs.receipts, "bg").unwrap(), a);
        assert_eq!(run(&dirs, Action::Install, &a).0.unwrap_err(), "already installed");
        run(&dirs, Action::Uninstall, &a).0.unwrap();
        assert!(!dirs.receipts.join("bg.json").exists());
        assert!(!is_installed(&a, &ids, &dirs.receipts));
    }

    #[test]
    fn is_installed_follows_the_registry_for_tile_apps_and_the_receipt_otherwise() {
        let dirs = temp("isinst");
        let tiled = app("t", &[], &[]);
        let ids: HashSet<String> = ["t".to_string()].into();
        assert!(is_installed(&tiled, &ids, &dirs.receipts));
        assert!(!is_installed(&tiled, &HashSet::new(), &dirs.receipts));
        // a registry entry with the same id does not make a tile-less app installed
        let bg = notile("t");
        assert!(!is_installed(&bg, &ids, &dirs.receipts));
    }
}
