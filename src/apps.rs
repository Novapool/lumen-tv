// App lifecycle. Two halves:
//   (a) `Lifecycle`: a pure planner. Events in, `Step`s out, no I/O, no
//       platform imports (window ids are plain u64). Unit-tested below.
//   (b) Process control: spawn, reaper, descendant lookup, terminate, and a
//       light poll for apps whose launcher exits at once.
//
// Registry field `process` (the real app's executable): for entries that have
// it, the launcher's exit is NOT an app exit. Liveness then comes from a ~1 s
// poll that looks for the process (`Lifecycle::poll_targets` + `process_poll`),
// and `terminate` targets those PIDs as well as the launcher's descendants.
// Auto-return on "last bound window closed" applies to every app.

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use crate::registry::{Background, Entry, Kind};

/// app_id Lumen gives its own window (`slint::set_xdg_app_id`).
pub const LUMEN_APP_ID: &str = "lumen-tv";
/// An app without an `app_id` binds the first new window opened this soon
/// after launch; a `process` app that never shows up in this long failed.
pub const BIND_WINDOW_MS: u64 = 30_000;
/// A `process` app may close its last window and open another (Steam Link
/// swaps its menu window for the stream window, ~2 s apart). If its process
/// is still alive this long after with no window, it is ended.
pub const WINDOWLESS_MS: u64 = 10_000;
/// `{id}` gives each web app its own Chromium profile (otherwise a second
/// launch hands off to the running browser and exits) and its own Wayland
/// app_id, `lumen-<id>`, so two web apps' windows can be told apart.
pub const DEFAULT_BROWSER: &str = "chromium --kiosk --ozone-platform=wayland --noerrdialogs --class=lumen-{id} --user-data-dir=\"$HOME/.local/share/lumen-tv/browser/{id}\" {url}";

// ---------------------------------------------------------------- (a) planner

/// What the planner needs to know about a registry entry.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub id: String,
    pub exclusive: bool,
    pub keep_alive: bool,
    pub app_id: Option<String>,
    /// Registry field `process`, as written (`~` not yet expanded).
    pub process: Option<String>,
}

impl From<&Entry> for Spec {
    fn from(e: &Entry) -> Spec {
        Spec {
            id: e.id.clone(),
            exclusive: e.exclusive,
            keep_alive: e.background == Background::KeepAlive,
            app_id: e.app_id.clone(),
            process: e.process.clone(),
        }
    }
}

/// A window as the planner sees it (main converts `platform::Toplevel`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowInfo {
    pub id: u64,
    pub app_id: String,
    pub activated: bool,
    pub fullscreen: bool,
}

impl WindowInfo {
    fn is_lumen(&self) -> bool {
        self.app_id == LUMEN_APP_ID
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Foreground {
    Lumen,
    App(String),
}

/// What the app being terminated looked like, so main can close its windows
/// and kill its processes after the planner has already forgotten it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Termination {
    pub id: String,
    pub pid: Option<u32>,
    pub windows: Vec<u64>,
    pub process: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Step {
    Spawn(String),
    Activate(u64),
    ActivateLumen,
    Fullscreen(u64),
    Terminate(Termination),
}

#[allow(dead_code)] // M3.5 Settings
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppSnapshot {
    pub id: String,
    pub pid: Option<u32>,
    pub windows: Vec<u64>,
    /// Running but no window bound yet.
    pub launching: bool,
}

/// Read-only view for the M3.5 Settings screen.
#[allow(dead_code)] // M3.5 Settings
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub foreground: Foreground,
    pub apps: Vec<AppSnapshot>,
}

#[derive(Default)]
struct AppState {
    pid: Option<u32>,
    windows: Vec<u64>,
    since_ms: u64,
    /// `process` apps: the process has been found at least once.
    seen: bool,
    /// `process` apps: when the last window closed while the process lived on.
    windowless_since: Option<u64>,
}

/// What a Terminate left behind: windows and processes that are still dying
/// and must not be mistaken for a new run of the same app. The simplest safe
/// answer to a quick re-select (rather than deferring the Spawn until the
/// kill completes, which needs a completion callback and makes the tile feel
/// dead for up to 5 s) is to let the new run start at once and ignore these:
/// toplevel ids are monotonic, so a retired window id can never be a new
/// window, and a retired PID is never the new run's process.
#[derive(Default)]
struct Retiring {
    windows: Vec<u64>,
    pids: Vec<u32>,
}

pub struct Lifecycle {
    specs: Vec<Spec>,
    running: BTreeMap<String, AppState>,
    retiring: BTreeMap<String, Retiring>,
    fg: Foreground,
}

impl Lifecycle {
    pub fn new(specs: Vec<Spec>) -> Lifecycle {
        Lifecycle { specs, running: BTreeMap::new(), retiring: BTreeMap::new(), fg: Foreground::Lumen }
    }

    fn spec(&self, id: &str) -> Option<&Spec> {
        self.specs.iter().find(|s| s.id == id)
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.running.contains_key(id)
    }

    #[allow(dead_code)] // tests now, Settings in M3.5
    pub fn foreground(&self) -> &Foreground {
        &self.fg
    }

    #[allow(dead_code)] // M3.5 Settings
    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            foreground: self.fg.clone(),
            apps: self
                .running
                .iter()
                .map(|(id, st)| AppSnapshot {
                    id: id.clone(),
                    pid: st.pid,
                    windows: st.windows.clone(),
                    launching: st.windows.is_empty(),
                })
                .collect(),
        }
    }

    /// (id, process) of every running or launching app that needs the poll.
    pub fn poll_targets(&self) -> Vec<(String, String)> {
        self.running
            .keys()
            .filter_map(|id| Some((id.clone(), self.spec(id)?.process.clone()?)))
            .collect()
    }

    /// Main reports the launcher's PID after `spawn` succeeds.
    pub fn set_pid(&mut self, id: &str, pid: u32) {
        if let Some(st) = self.running.get_mut(id) {
            st.pid = Some(pid);
        }
    }

    /// Main reports the PIDs `collect_targets` returned for a Terminate, so the
    /// poll can tell the dying run's process from a new run's.
    pub fn retire_pids(&mut self, id: &str, pids: &[u32]) {
        // Only `process` apps are polled, so only they have PIDs to ignore.
        if self.spec(id).is_some_and(|s| s.process.is_some()) {
            self.retiring.entry(id.to_string()).or_default().pids.extend_from_slice(pids);
        }
    }

    fn termination(&mut self, id: &str, st: AppState) -> Termination {
        self.retiring.entry(id.to_string()).or_default().windows.extend_from_slice(&st.windows);
        Termination {
            id: id.to_string(),
            pid: st.pid,
            windows: st.windows,
            process: self.spec(id).and_then(|s| s.process.clone()),
        }
    }

    /// Forget an app that is gone; send the screen back to Lumen if it was in front.
    fn finish(&mut self, id: &str) -> Vec<Step> {
        if self.running.remove(id).is_none() {
            return vec![];
        }
        if self.fg == Foreground::App(id.to_string()) {
            self.fg = Foreground::Lumen;
            return vec![Step::ActivateLumen];
        }
        vec![]
    }

    pub fn select(&mut self, id: &str, now_ms: u64) -> Vec<Step> {
        let Some(spec) = self.spec(id).cloned() else { return vec![] };
        if let Some(st) = self.running.get(id) {
            // Launching: a second select does nothing. Running: bring it back.
            return match st.windows.last() {
                Some(&w) => {
                    self.fg = Foreground::App(id.to_string());
                    vec![Step::Activate(w)]
                }
                None => vec![],
            };
        }
        let mut steps = Vec::new();
        if spec.exclusive {
            let others: Vec<String> = self.running.keys().cloned().collect();
            for other in others {
                let st = self.running.remove(&other).unwrap();
                if self.fg == Foreground::App(other.clone()) {
                    self.fg = Foreground::Lumen;
                }
                steps.push(Step::Terminate(self.termination(&other, st)));
            }
        }
        self.running.insert(id.to_string(), AppState { since_ms: now_ms, ..Default::default() });
        steps.push(Step::Spawn(id.to_string()));
        steps
    }

    /// Home pressed. `lumen_activated`: Lumen's own window is the active one.
    /// If it is not, something else has the screen that the planner doesn't
    /// know about (an unbound window, a launch still unbound), so Home always
    /// brings Lumen back and ends close-on-exit apps that are still launching.
    pub fn home(&mut self, lumen_activated: bool) -> Vec<Step> {
        let Foreground::App(id) = self.fg.clone() else {
            if lumen_activated {
                return vec![];
            }
            let mut steps = vec![Step::ActivateLumen];
            let launching: Vec<String> = self
                .running
                .iter()
                .filter(|(id, st)| st.windows.is_empty() && !self.spec(id).is_some_and(|s| s.keep_alive))
                .map(|(id, _)| id.clone())
                .collect();
            for id in launching {
                let st = self.running.remove(&id).unwrap();
                steps.push(Step::Terminate(self.termination(&id, st)));
            }
            return steps;
        };
        self.fg = Foreground::Lumen;
        let keep_alive = self.spec(&id).is_some_and(|s| s.keep_alive);
        let mut steps = vec![Step::ActivateLumen];
        if !keep_alive {
            if let Some(st) = self.running.remove(&id) {
                steps.push(Step::Terminate(self.termination(&id, st)));
            }
        }
        steps
    }

    pub fn window_opened(&mut self, w: &WindowInfo, now_ms: u64) -> Vec<Step> {
        self.observe(w, now_ms, true)
    }

    pub fn window_changed(&mut self, w: &WindowInfo, now_ms: u64) -> Vec<Step> {
        self.observe(w, now_ms, false)
    }

    /// Which running app does an unbound window belong to?
    fn bind_target(&self, w: &WindowInfo, now_ms: u64, opened: bool) -> Option<String> {
        let launching = |id: &String| self.running.get(id).is_some_and(|st| st.windows.is_empty());
        // A `process` app's real process may open its own windows, named
        // after it (Steam Link's stream window has app_id "shell").
        let process_name = |s: &Spec| s.process.as_deref().and_then(|p| p.rsplit('/').next()).map(str::to_string);
        let by_app_id = |s: &&Spec| {
            self.running.contains_key(&s.id)
                && !w.app_id.is_empty()
                && (s.app_id.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&w.app_id))
                    || process_name(s).is_some_and(|n| n == w.app_id))
        };
        let mut matches: Vec<&Spec> = self.specs.iter().filter(by_app_id).collect();
        // Prefer the app that is still waiting for its first window.
        matches.sort_by_key(|s| !launching(&s.id));
        if let Some(s) = matches.first() {
            return Some(s.id.clone());
        }
        // No app_id on the entry: the first new window within the bind period.
        if !opened || self.specs.iter().any(|s| s.app_id.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&w.app_id))) {
            return None;
        }
        self.specs
            .iter()
            .filter(|s| s.app_id.is_none() && launching(&s.id))
            .filter(|s| now_ms.saturating_sub(self.running[&s.id].since_ms) <= BIND_WINDOW_MS)
            .min_by_key(|s| self.running[&s.id].since_ms)
            .map(|s| s.id.clone())
    }

    fn observe(&mut self, w: &WindowInfo, now_ms: u64, opened: bool) -> Vec<Step> {
        if w.is_lumen() {
            if w.activated {
                self.fg = Foreground::Lumen;
            }
            return vec![];
        }
        if self.retiring.values().any(|r| r.windows.contains(&w.id)) {
            return vec![]; // a window of a run that is being terminated
        }
        if let Some(id) = self.running.iter().find(|(_, st)| st.windows.contains(&w.id)).map(|(id, _)| id.clone()) {
            if w.activated {
                self.fg = Foreground::App(id);
            }
            return vec![];
        }
        let Some(id) = self.bind_target(w, now_ms, opened) else { return vec![] };
        let st = self.running.get_mut(&id).unwrap();
        let first = st.windows.is_empty();
        st.windows.push(w.id);
        st.windowless_since = None;
        if first {
            self.fg = Foreground::App(id);
            if !w.fullscreen {
                return vec![Step::Fullscreen(w.id)];
            }
        } else if w.activated {
            self.fg = Foreground::App(id);
        }
        vec![]
    }

    /// The last bound window of an app closing ends the app, except for a
    /// `process` app: the poll decides when it is gone, and it may open a new
    /// window (see `WINDOWLESS_MS`).
    pub fn window_closed(&mut self, window: u64, now_ms: u64) -> Vec<Step> {
        self.retiring.retain(|_, r| {
            r.windows.retain(|&w| w != window);
            !(r.windows.is_empty() && r.pids.is_empty())
        });
        let Some(id) = self.running.iter().find(|(_, st)| st.windows.contains(&window)).map(|(id, _)| id.clone()) else {
            return vec![];
        };
        self.running.get_mut(&id).unwrap().windows.retain(|&w| w != window);
        if !self.running[&id].windows.is_empty() {
            return vec![];
        }
        if self.spec(&id).is_some_and(|s| s.process.is_some()) {
            self.running.get_mut(&id).unwrap().windowless_since = Some(now_ms);
            return vec![];
        }
        self.end_windowless(&id)
    }

    /// Ends an app that has no window left: back to Lumen if it was in front,
    /// then sweep the process, which may outlive its window.
    fn end_windowless(&mut self, id: &str) -> Vec<Step> {
        let id = id.to_string();
        let st = self.running.remove(&id).unwrap();
        let mut steps = Vec::new();
        if self.fg == Foreground::App(id.clone()) {
            self.fg = Foreground::Lumen;
            steps.push(Step::ActivateLumen);
        }
        steps.push(Step::Terminate(self.termination(&id, st)));
        steps
    }

    /// The launcher process (the PID from `set_pid`) exited with `code`.
    pub fn process_exited(&mut self, id: &str, pid: u32, code: i32) -> Vec<Step> {
        let Some(st) = self.running.get(id) else { return vec![] };
        if st.pid.is_some_and(|p| p != pid) {
            return vec![]; // a stale reaper from an earlier run of this app
        }
        let launching = st.windows.is_empty();
        if self.spec(id).is_some_and(|s| s.process.is_some()) {
            // The launcher exiting is not the app exiting; the poll decides.
            // A failing launcher that never produced the app is a failed launch.
            if code != 0 && launching && !st.seen {
                return self.launch_failed(id);
            }
            return vec![];
        }
        // No `process`: the launcher is the app. Dying while still launching
        // is a launch failure; dying later is the app exiting. Both end it.
        self.finish(id)
    }

    /// `spawn` itself failed, or the app never came up.
    pub fn launch_failed(&mut self, id: &str) -> Vec<Step> {
        self.finish(id)
    }

    /// One poll result for a `process` app: the PIDs that match its process.
    /// PIDs of a run being terminated don't count; once none of them is left
    /// they are forgotten.
    pub fn process_poll(&mut self, id: &str, pids: &[u32], now_ms: u64) -> Vec<Step> {
        if !self.spec(id).is_some_and(|s| s.process.is_some()) {
            return vec![];
        }
        let mut found = !pids.is_empty();
        if let Some(r) = self.retiring.get_mut(id) {
            found = pids.iter().any(|p| !r.pids.contains(p));
            if !pids.iter().any(|p| r.pids.contains(p)) {
                r.pids.clear(); // the old run is gone
            }
            if r.windows.is_empty() && r.pids.is_empty() {
                self.retiring.remove(id);
            }
        }
        let Some(st) = self.running.get_mut(id) else { return vec![] };
        if found {
            st.seen = true;
            if st.windowless_since.is_some_and(|t| now_ms.saturating_sub(t) > WINDOWLESS_MS) {
                return self.end_windowless(id);
            }
            return vec![];
        }
        let never_came_up = !st.seen && st.windows.is_empty() && now_ms.saturating_sub(st.since_ms) > BIND_WINDOW_MS;
        if st.seen || never_came_up {
            return self.finish(id);
        }
        vec![]
    }
}

// ------------------------------------------------------- (b) process control

fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

// Registry ids are [A-Za-z0-9_-], so `{id}` needs no quoting.
fn browser_line(browser_cmd: &str, id: &str, url: &str) -> String {
    let cmd = browser_cmd.replace("{id}", id);
    if cmd.contains("{url}") {
        cmd.replace("{url}", &shell_quote(url))
    } else {
        format!("{cmd} {}", shell_quote(url))
    }
}

/// `$LUMEN_BROWSER` or the default kiosk browser command.
pub fn browser_cmd() -> String {
    std::env::var("LUMEN_BROWSER").ok().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| DEFAULT_BROWSER.to_string())
}

pub fn program_on_path(program: &str) -> bool {
    if program.contains('/') {
        return Path::new(program).is_file();
    }
    std::env::var_os("PATH").is_some_and(|p| std::env::split_paths(&p).any(|d| d.join(program).is_file()))
}

/// The shell line to `exec` for an entry. `on_path` is injected for tests.
pub fn command_line(entry: &Entry, browser_cmd: &str, on_path: &dyn Fn(&str) -> bool) -> Result<String, String> {
    match entry.kind {
        Kind::Url => {
            let url = entry.url.as_deref().ok_or("no url")?;
            Ok(browser_line(browser_cmd, &entry.id, url))
        }
        Kind::Command => {
            let launch = entry.launch.as_deref().ok_or("no launch line")?.trim();
            let program = launch.split_whitespace().next().ok_or("empty launch line")?;
            if on_path(program) {
                return Ok(launch.to_string());
            }
            match &entry.fallback_url {
                Some(url) => Ok(browser_line(browser_cmd, &entry.id, url)),
                None => Err(format!("{program}: not found on PATH")),
            }
        }
    }
}

fn exit_code(status: std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status.code().or_else(|| status.signal().map(|s| -s)).unwrap_or(-1)
}

/// Starts the app as `sh -c 'exec ...'` in its own process group, with
/// stdout/stderr inherited (journal) and Lumen's session environment. A
/// reaper thread calls `on_exit(id, pid, code)` when the launcher exits.
pub fn spawn(
    entry: &Entry,
    browser_cmd: &str,
    on_exit: impl FnOnce(String, u32, i32) + Send + 'static,
) -> Result<u32, String> {
    use std::os::unix::process::CommandExt;
    let line = command_line(entry, browser_cmd, &program_on_path)?;
    let mut child = Command::new("sh")
        .arg("-c")
        .arg(format!("exec {line}"))
        .stdin(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|e| format!("spawn failed: {e}"))?;
    let pid = child.id();
    let id = entry.id.clone();
    std::thread::spawn(move || {
        let code = child.wait().map(exit_code).unwrap_or(-1);
        on_exit(id, pid, code);
    });
    Ok(pid)
}

/// One line of the /proc process table.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Proc {
    pub pid: u32,
    pub ppid: u32,
    pub comm: String,
}

/// Parses `/proc/<pid>/stat`: `pid (comm) state ppid ...`. The comm may hold
/// spaces and parentheses, so split at the last `)`.
pub fn parse_stat(line: &str) -> Option<Proc> {
    let open = line.find('(')?;
    let close = line.rfind(')')?;
    if close < open {
        return None;
    }
    let pid = line[..open].trim().parse().ok()?;
    let mut rest = line[close + 1..].split_whitespace();
    rest.next()?; // state
    let ppid = rest.next()?.parse().ok()?;
    Some(Proc { pid, ppid, comm: line[open + 1..close].to_string() })
}

/// `root` and everything below it (breadth first). Follows ppid links, since
/// process groups miss Flatpak's bwrap, which calls setsid.
pub fn descendants(root: u32, table: &[Proc]) -> Vec<u32> {
    let mut out = vec![root];
    let mut seen: HashSet<u32> = out.iter().copied().collect();
    let mut i = 0;
    while i < out.len() {
        let parent = out[i];
        for p in table.iter().filter(|p| p.ppid == parent) {
            if seen.insert(p.pid) {
                out.push(p.pid);
            }
        }
        i += 1;
    }
    out
}

pub fn read_table() -> Vec<Proc> {
    let Ok(rd) = std::fs::read_dir("/proc") else { return vec![] };
    rd.flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())))
        .filter_map(|e| parse_stat(&std::fs::read_to_string(e.path().join("stat")).ok()?))
        .collect()
}

pub fn expand_tilde(spec: &str, home: &str) -> String {
    match spec.strip_prefix('~') {
        Some("") => home.to_string(),
        Some(rest) if rest.starts_with('/') => format!("{}{rest}", home.trim_end_matches('/')),
        _ => spec.to_string(),
    }
}

/// Does a process match a registry `process` value (already `~`-expanded)?
/// With a `/` it is the executable path; without, the comm (the kernel keeps
/// 15 bytes of it).
pub fn matches_process(spec: &str, exe: Option<&str>, comm: &str) -> bool {
    if spec.contains('/') {
        exe.is_some_and(|e| e == spec)
    } else {
        comm == spec.get(..15).unwrap_or(spec)
    }
}

/// PIDs of every running process that matches `process`.
pub fn find_pids(process: &str, table: &[Proc]) -> Vec<u32> {
    let home = std::env::var("HOME").unwrap_or_default();
    let spec = expand_tilde(process, &home);
    let by_path = spec.contains('/');
    table
        .iter()
        .filter(|p| {
            let exe = if by_path {
                std::fs::read_link(format!("/proc/{}/exe", p.pid)).ok().map(|e| e.to_string_lossy().into_owned())
            } else {
                None
            };
            matches_process(&spec, exe.as_deref(), &p.comm)
        })
        .map(|p| p.pid)
        .collect()
}

/// Everything to stop for an app: the launcher and its descendants, plus any
/// process matching `process` and its descendants.
pub fn collect_targets(launcher: Option<u32>, process: Option<&str>) -> Vec<u32> {
    let table = read_table();
    let mut out: Vec<u32> = Vec::new();
    let mut roots: Vec<u32> = launcher.into_iter().collect();
    if let Some(p) = process {
        roots.extend(find_pids(p, &table));
    }
    for r in roots {
        for pid in descendants(r, &table) {
            if !out.contains(&pid) {
                out.push(pid);
            }
        }
    }
    out
}

#[cfg(target_os = "linux")]
fn alive(pid: u32) -> bool {
    pid > 1 && unsafe { libc::kill(pid as i32, 0) } == 0
}

#[cfg(target_os = "linux")]
fn signal(pid: u32, sig: i32) {
    if pid > 1 {
        unsafe { libc::kill(pid as i32, sig) };
    }
}

/// On its own thread: wait `pre_delay` (main closes the app's windows during
/// it), SIGTERM whatever is still alive, and SIGKILL the survivors 3 s later.
#[cfg(target_os = "linux")]
pub fn terminate(pids: Vec<u32>, pre_delay: Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(pre_delay);
        for &p in pids.iter().filter(|&&p| alive(p)) {
            signal(p, libc::SIGTERM);
        }
        std::thread::sleep(Duration::from_secs(3));
        for &p in pids.iter().filter(|&&p| alive(p)) {
            signal(p, libc::SIGKILL);
        }
    });
}

/// Polls about once a second for the `process` of each target (`Lifecycle::
/// poll_targets`), reporting `(id, pids)`: every PID that matches. Does nothing while there are no
/// targets, so it costs nothing unless such an app is running.
pub fn start_poller(
    targets: impl Fn() -> Vec<(String, String)> + Send + 'static,
    report: impl Fn(String, Vec<u32>) + Send + 'static,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(1));
        let wanted = targets();
        if wanted.is_empty() {
            continue;
        }
        let table = read_table();
        for (id, process) in wanted {
            let pids = find_pids(&process, &table);
            report(id, pids);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(id: &str) -> Spec {
        Spec { id: id.into(), exclusive: false, keep_alive: false, app_id: Some(id.into()), process: None }
    }
    fn keep(id: &str) -> Spec {
        Spec { keep_alive: true, ..spec(id) }
    }
    fn excl(id: &str) -> Spec {
        Spec { exclusive: true, ..spec(id) }
    }
    fn proc_app(id: &str) -> Spec {
        Spec { exclusive: true, process: Some("~/real/shell".into()), ..spec(id) }
    }
    fn win(id: u64, app_id: &str) -> WindowInfo {
        WindowInfo { id, app_id: app_id.into(), activated: true, fullscreen: true }
    }
    fn lumen_win(activated: bool) -> WindowInfo {
        WindowInfo { id: 1, app_id: LUMEN_APP_ID.into(), activated, fullscreen: true }
    }
    fn term(id: &str, pid: Option<u32>, windows: &[u64], process: Option<&str>) -> Step {
        Step::Terminate(Termination { id: id.into(), pid, windows: windows.to_vec(), process: process.map(String::from) })
    }
    /// Select `id`, give it pid 100 and one window 50 with its own app_id.
    fn up(lc: &mut Lifecycle, id: &str) {
        lc.select(id, 0);
        lc.set_pid(id, 100);
        lc.window_opened(&win(50, id), 10);
    }

    // select
    #[test]
    fn select_spawns_a_new_app() {
        let mut lc = Lifecycle::new(vec![spec("a")]);
        assert_eq!(lc.select("a", 0), vec![Step::Spawn("a".into())]);
        assert!(lc.is_running("a"));
        assert_eq!(lc.select("nope", 0), vec![]);
    }

    #[test]
    fn exclusive_terminates_others_before_the_spawn() {
        let mut lc = Lifecycle::new(vec![keep("j"), keep("p"), excl("s")]);
        up(&mut lc, "j");
        lc.select("p", 20);
        lc.set_pid("p", 200);
        let steps = lc.select("s", 30);
        assert_eq!(steps, vec![term("j", Some(100), &[50], None), term("p", Some(200), &[], None), Step::Spawn("s".into())]);
        assert!(!lc.is_running("j") && !lc.is_running("p") && lc.is_running("s"));
        assert_eq!(lc.foreground(), &Foreground::Lumen);
    }

    #[test]
    fn non_exclusive_leaves_others_running() {
        let mut lc = Lifecycle::new(vec![keep("j"), keep("p")]);
        up(&mut lc, "j");
        assert_eq!(lc.select("p", 20), vec![Step::Spawn("p".into())]);
        assert!(lc.is_running("j"));
    }

    #[test]
    fn double_select_while_launching_does_nothing() {
        let mut lc = Lifecycle::new(vec![spec("a")]);
        lc.select("a", 0);
        assert_eq!(lc.select("a", 5), vec![]);
        assert_eq!(lc.snapshot().apps.len(), 1);
    }

    #[test]
    fn select_running_app_activates_its_latest_window() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        lc.window_opened(&win(51, "j"), 20);
        lc.window_changed(&lumen_win(true), 30);
        assert_eq!(lc.foreground(), &Foreground::Lumen);
        assert_eq!(lc.select("j", 40), vec![Step::Activate(51)]);
        assert_eq!(lc.foreground(), &Foreground::App("j".into()));
    }

    // home
    #[test]
    fn home_with_lumen_in_front_does_nothing() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        assert_eq!(lc.home(true), vec![]);
        up(&mut lc, "j");
        lc.window_changed(&lumen_win(true), 30);
        assert_eq!(lc.home(true), vec![]);
    }

    #[test]
    fn home_keep_alive_returns_and_keeps_running() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        assert_eq!(lc.home(true), vec![Step::ActivateLumen]);
        assert!(lc.is_running("j"));
        assert_eq!(lc.foreground(), &Foreground::Lumen);
    }

    #[test]
    fn home_close_on_exit_returns_first_then_terminates() {
        let mut lc = Lifecycle::new(vec![excl("k")]);
        up(&mut lc, "k");
        assert_eq!(lc.home(true), vec![Step::ActivateLumen, term("k", Some(100), &[50], None)]);
        assert!(!lc.is_running("k"));
    }

    // binding
    #[test]
    fn binds_by_app_id_and_fullscreens_when_needed() {
        let mut lc = Lifecycle::new(vec![spec("a"), spec("b")]);
        lc.select("a", 0);
        let w = WindowInfo { id: 7, app_id: "A".into(), activated: true, fullscreen: false }; // case-insensitive
        assert_eq!(lc.window_opened(&w, 5), vec![Step::Fullscreen(7)]);
        assert_eq!(lc.foreground(), &Foreground::App("a".into()));
        assert_eq!(lc.snapshot().apps[0].windows, vec![7]);
        assert!(!lc.snapshot().apps[0].launching);
    }

    #[test]
    fn app_id_apps_ignore_other_windows() {
        let mut lc = Lifecycle::new(vec![spec("a")]);
        lc.select("a", 0);
        assert_eq!(lc.window_opened(&win(7, "something-else"), 5), vec![]);
        assert!(lc.snapshot().apps[0].launching);
    }

    #[test]
    fn binds_first_new_window_within_30s_without_app_id() {
        let anon = Spec { app_id: None, ..spec("x") };
        let mut lc = Lifecycle::new(vec![anon.clone()]);
        lc.select("x", 1_000);
        assert_eq!(lc.window_opened(&win(9, "whatever"), 30_999), vec![]);
        assert_eq!(lc.snapshot().apps[0].windows, vec![9]);
        // a second window is not "the first"
        lc.window_opened(&win(10, "other"), 31_000);
        assert_eq!(lc.snapshot().apps[0].windows, vec![9]);

        let mut late = Lifecycle::new(vec![anon]);
        late.select("x", 1_000);
        late.window_opened(&win(9, "whatever"), 31_001);
        assert!(late.snapshot().apps[0].launching, "31 s after launch is too late");
    }

    #[test]
    fn late_app_id_still_binds_on_change() {
        let mut lc = Lifecycle::new(vec![spec("a")]);
        lc.select("a", 0);
        lc.window_opened(&win(7, ""), 5);
        assert!(lc.snapshot().apps[0].launching);
        lc.window_changed(&win(7, "a"), 6);
        assert_eq!(lc.snapshot().apps[0].windows, vec![7]);
    }

    #[test]
    fn lumens_own_window_is_never_bound() {
        let anon = Spec { app_id: None, ..spec("x") };
        let mut lc = Lifecycle::new(vec![anon]);
        lc.select("x", 0);
        lc.window_opened(&lumen_win(false), 1);
        assert!(lc.snapshot().apps[0].launching);
    }

    // foreground truth
    #[test]
    fn lumens_activation_sets_the_foreground() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        assert_eq!(lc.foreground(), &Foreground::App("j".into()));
        lc.window_changed(&lumen_win(false), 20);
        assert_eq!(lc.foreground(), &Foreground::App("j".into()), "deactivated Lumen changes nothing");
        lc.window_changed(&lumen_win(true), 30);
        assert_eq!(lc.foreground(), &Foreground::Lumen);
        lc.window_changed(&win(50, "j"), 40);
        assert_eq!(lc.foreground(), &Foreground::App("j".into()));
    }

    // auto-return
    #[test]
    fn launcher_exit_in_front_returns_to_lumen() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        assert_eq!(lc.process_exited("j", 100, 0), vec![Step::ActivateLumen]);
        assert!(!lc.is_running("j"));
        assert_eq!(lc.foreground(), &Foreground::Lumen);
    }

    #[test]
    fn exit_of_a_background_app_does_not_steal_the_screen() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        lc.window_changed(&lumen_win(true), 20);
        assert_eq!(lc.process_exited("j", 100, 0), vec![]);
        assert!(!lc.is_running("j"));
    }

    #[test]
    fn stale_exit_from_an_earlier_run_is_ignored() {
        let mut lc = Lifecycle::new(vec![excl("k")]);
        up(&mut lc, "k");
        lc.home(true);
        lc.select("k", 100);
        lc.set_pid("k", 300);
        assert_eq!(lc.process_exited("k", 100, 0), vec![]);
        assert!(lc.is_running("k"));
    }

    #[test]
    fn last_window_closed_returns_and_sweeps_the_process() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        lc.window_opened(&win(51, "j"), 20);
        assert_eq!(lc.window_closed(50, 0), vec![]);
        assert!(lc.is_running("j"));
        assert_eq!(lc.window_closed(51, 0), vec![Step::ActivateLumen, term("j", Some(100), &[], None)]);
        assert!(!lc.is_running("j"));
        assert_eq!(lc.window_closed(51, 0), vec![]);
    }

    #[test]
    fn last_window_closed_in_the_background_skips_the_return() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        lc.window_changed(&lumen_win(true), 20);
        assert_eq!(lc.window_closed(50, 0), vec![term("j", Some(100), &[], None)]);
    }

    // launch failure
    #[test]
    fn launcher_dying_while_launching_is_a_launch_failure() {
        let mut lc = Lifecycle::new(vec![spec("a")]);
        lc.select("a", 0);
        lc.set_pid("a", 100);
        assert_eq!(lc.process_exited("a", 100, 127), vec![]);
        assert!(!lc.is_running("a"));
        assert_eq!(lc.foreground(), &Foreground::Lumen);
        assert_eq!(lc.select("a", 50), vec![Step::Spawn("a".into())], "can be launched again");
    }

    #[test]
    fn spawn_error_is_a_launch_failure() {
        let mut lc = Lifecycle::new(vec![spec("a")]);
        lc.select("a", 0);
        assert_eq!(lc.launch_failed("a"), vec![]);
        assert!(!lc.is_running("a"));
    }

    // `process` apps (launcher exits immediately)
    #[test]
    fn process_app_launcher_exit_is_not_an_app_exit() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        lc.select("s", 0);
        lc.set_pid("s", 100);
        assert_eq!(lc.process_exited("s", 100, 0), vec![], "launching: no auto-return");
        assert!(lc.is_running("s"));
        lc.window_opened(&win(50, "s"), 3_000);
        assert_eq!(lc.foreground(), &Foreground::App("s".into()));
        assert_eq!(lc.process_poll("s", &[7], 4_000), vec![]);
        assert!(lc.is_running("s"));
    }

    #[test]
    fn process_app_exit_found_by_poll_returns_to_lumen() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        lc.select("s", 0);
        lc.set_pid("s", 100);
        lc.process_exited("s", 100, 0);
        lc.window_opened(&win(50, "s"), 3_000);
        lc.process_poll("s", &[7], 4_000);
        assert_eq!(lc.process_poll("s", &[], 5_000), vec![Step::ActivateLumen]);
        assert!(!lc.is_running("s"));
    }

    #[test]
    fn process_app_not_yet_seen_is_not_gone() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        lc.select("s", 0);
        lc.set_pid("s", 100);
        lc.process_exited("s", 100, 0);
        assert_eq!(lc.process_poll("s", &[], 5_000), vec![]);
        assert!(lc.is_running("s"));
    }

    #[test]
    fn process_app_that_never_appears_fails_after_30s() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        lc.select("s", 1_000);
        lc.process_exited("s", 0, 0);
        assert_eq!(lc.process_poll("s", &[], 31_000), vec![]);
        assert!(lc.is_running("s"));
        assert_eq!(lc.process_poll("s", &[], 31_001), vec![]);
        assert!(!lc.is_running("s"));
    }

    #[test]
    fn process_app_failing_launcher_is_a_launch_failure() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        lc.select("s", 0);
        lc.set_pid("s", 100);
        assert_eq!(lc.process_exited("s", 100, 127), vec![]);
        assert!(!lc.is_running("s"));
    }

    #[test]
    fn process_app_survives_swapping_its_window() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        up(&mut lc, "s");
        lc.process_exited("s", 100, 0);
        lc.process_poll("s", &[7], 1_000);
        assert_eq!(lc.window_closed(50, 2_000), vec![], "the poll decides, not the window");
        assert!(lc.is_running("s"));
        lc.window_changed(&lumen_win(true), 2_000);
        assert_eq!(lc.process_poll("s", &[7], 3_000), vec![]);
        lc.window_opened(&win(52, "shell"), 4_000);
        assert_eq!(lc.foreground(), &Foreground::App("s".into()), "the process's own window binds");
        assert_eq!(lc.process_poll("s", &[7], 60_000), vec![], "a bound window clears the timer");
    }

    #[test]
    fn process_app_exiting_after_its_window_closed_returns_to_lumen() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        up(&mut lc, "s");
        lc.process_poll("s", &[7], 1_000);
        lc.window_closed(50, 2_000);
        assert_eq!(lc.process_poll("s", &[], 3_000), vec![Step::ActivateLumen]);
        assert!(!lc.is_running("s"));
    }

    #[test]
    fn process_app_alive_without_a_window_is_ended_after_the_grace() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        up(&mut lc, "s");
        lc.process_poll("s", &[7], 1_000);
        lc.window_closed(50, 2_000);
        assert_eq!(lc.process_poll("s", &[7], 2_000 + WINDOWLESS_MS), vec![]);
        assert_eq!(
            lc.process_poll("s", &[7], 2_001 + WINDOWLESS_MS),
            vec![Step::ActivateLumen, term("s", Some(100), &[], Some("~/real/shell"))]
        );
        assert!(!lc.is_running("s"));
    }

    #[test]
    fn process_app_terminate_carries_the_process_spec() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        up(&mut lc, "s");
        assert_eq!(lc.home(true), vec![Step::ActivateLumen, term("s", Some(100), &[50], Some("~/real/shell"))]);
    }

    #[test]
    fn poll_targets_only_lists_running_process_apps() {
        let mut lc = Lifecycle::new(vec![proc_app("s"), keep("j")]);
        assert!(lc.poll_targets().is_empty());
        lc.select("j", 0);
        assert!(lc.poll_targets().is_empty());
        lc.select("s", 1);
        assert_eq!(lc.poll_targets(), vec![("s".to_string(), "~/real/shell".to_string())]);
        lc.process_poll("s", &[], 99_999);
        assert!(lc.poll_targets().is_empty());
    }

    #[test]
    fn poll_ignores_apps_without_process() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        assert_eq!(lc.process_poll("j", &[], 99_999), vec![]);
        assert!(lc.is_running("j"));
    }


    // quick re-select after Home (review 3.1 critical)
    #[test]
    fn process_app_reselect_within_the_kill_window_ignores_the_dying_process() {
        let mut lc = Lifecycle::new(vec![proc_app("s")]);
        up(&mut lc, "s");
        lc.process_exited("s", 100, 0);
        lc.process_poll("s", &[7], 1_000);
        assert_eq!(lc.home(true), vec![Step::ActivateLumen, term("s", Some(100), &[50], Some("~/real/shell"))]);
        lc.retire_pids("s", &[7, 8]);
        assert_eq!(lc.select("s", 2_000), vec![Step::Spawn("s".into())]);
        lc.set_pid("s", 200);
        // the old shell is still alive: not the new run
        assert_eq!(lc.process_poll("s", &[7], 3_000), vec![]);
        // the old window closes late: not an event of the new run
        assert_eq!(lc.window_closed(50, 0), vec![]);
        lc.window_opened(&win(60, "s"), 3_500);
        assert_eq!(lc.foreground(), &Foreground::App("s".into()));
        // the old shell dies before the new one shows up: the launch is not dropped
        assert_eq!(lc.process_poll("s", &[], 5_000), vec![]);
        assert!(lc.is_running("s"));
        // the new shell appears, runs, and exits: that is the app exiting
        assert_eq!(lc.process_poll("s", &[9], 6_000), vec![]);
        assert_eq!(lc.process_poll("s", &[], 20_000), vec![Step::ActivateLumen]);
        assert!(!lc.is_running("s"));
    }

    #[test]
    fn app_id_app_reselect_ignores_the_dying_window() {
        let mut lc = Lifecycle::new(vec![spec("k")]);
        up(&mut lc, "k");
        assert_eq!(lc.home(true), vec![Step::ActivateLumen, term("k", Some(100), &[50], None)]);
        lc.retire_pids("k", &[100]);
        assert_eq!(lc.select("k", 2_000), vec![Step::Spawn("k".into())]);
        lc.set_pid("k", 200);
        // events of the old window must not bind to, or kill, the new run
        assert_eq!(lc.window_changed(&win(50, "k"), 2_500), vec![]);
        assert!(lc.snapshot().apps[0].launching);
        assert_eq!(lc.window_closed(50, 0), vec![]);
        assert!(lc.is_running("k"));
        // the new window binds
        lc.window_opened(&win(51, "k"), 3_000);
        assert_eq!(lc.foreground(), &Foreground::App("k".into()));
        assert_eq!(lc.snapshot().apps[0].windows, vec![51]);
    }

    // Home fallback
    #[test]
    fn home_without_lumen_activated_always_activates_lumen() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        assert_eq!(lc.home(false), vec![Step::ActivateLumen]);
        up(&mut lc, "j");
        lc.window_opened(&lumen_win(true), 20);
        // an unbound window has the screen while the planner says Lumen
        assert_eq!(lc.home(false), vec![Step::ActivateLumen]);
        assert!(lc.is_running("j"), "keep-alive stays");
    }

    #[test]
    fn home_without_lumen_activated_terminates_a_launching_close_on_exit_app() {
        let mut lc = Lifecycle::new(vec![spec("a"), keep("j")]);
        lc.select("a", 0);
        lc.set_pid("a", 100);
        lc.select("j", 1);
        assert_eq!(lc.home(false), vec![Step::ActivateLumen, term("a", Some(100), &[], None)]);
        assert!(!lc.is_running("a"));
        assert!(lc.is_running("j"), "a launching keep-alive app is left alone");
        assert_eq!(lc.home(true), vec![], "Lumen active and nothing in front: Home is a no-op");
    }

    // M3.5 hook
    #[test]
    fn snapshot_reports_foreground_and_apps() {
        let mut lc = Lifecycle::new(vec![keep("j"), keep("p")]);
        up(&mut lc, "j");
        lc.select("p", 20);
        lc.set_pid("p", 200);
        let s = lc.snapshot();
        assert_eq!(s.foreground, Foreground::App("j".into()));
        assert_eq!(
            s.apps,
            vec![
                AppSnapshot { id: "j".into(), pid: Some(100), windows: vec![50], launching: false },
                AppSnapshot { id: "p".into(), pid: Some(200), windows: vec![], launching: true },
            ]
        );
    }


    // --- Job 3.3 gap tests ---
    #[test]
    fn exclusive_with_nothing_running_only_spawns() {
        let mut lc = Lifecycle::new(vec![excl("s")]);
        assert_eq!(lc.select("s", 0), vec![Step::Spawn("s".into())]);
    }

    #[test]
    fn exclusive_with_one_app_in_front_terminates_it_and_drops_to_lumen() {
        let mut lc = Lifecycle::new(vec![keep("j"), excl("s")]);
        up(&mut lc, "j");
        assert_eq!(lc.foreground(), &Foreground::App("j".into()));
        assert_eq!(lc.select("s", 30), vec![term("j", Some(100), &[50], None), Step::Spawn("s".into())]);
        assert_eq!(lc.foreground(), &Foreground::Lumen);
    }

    #[test]
    fn home_while_launching_with_no_window_does_nothing() {
        let mut lc = Lifecycle::new(vec![excl("k")]);
        lc.select("k", 0);
        assert_eq!(lc.home(true), vec![]);
        assert!(lc.is_running("k"), "still launching");
        assert!(lc.snapshot().apps[0].launching);
    }

    #[test]
    fn window_from_an_app_that_was_not_launched_is_ignored() {
        let mut lc = Lifecycle::new(vec![spec("a"), keep("j")]);
        assert_eq!(lc.window_opened(&win(7, "a"), 5), vec![], "nothing running");
        assert_eq!(lc.foreground(), &Foreground::Lumen, "an activated stranger does not take the foreground");
        up(&mut lc, "j");
        assert_eq!(lc.window_opened(&win(8, "a"), 20), vec![], "a is known but not running");
        assert_eq!(lc.snapshot().apps.len(), 1);
        assert_eq!(lc.foreground(), &Foreground::App("j".into()));
    }

    #[test]
    fn anonymous_app_does_not_steal_a_window_that_belongs_to_another_entry() {
        let anon = Spec { app_id: None, ..spec("x") };
        let mut lc = Lifecycle::new(vec![anon, spec("a")]);
        lc.select("x", 0);
        assert_eq!(lc.window_opened(&win(7, "a"), 5), vec![]);
        assert!(lc.snapshot().apps[0].launching);
    }

    #[test]
    fn reselect_after_a_crash_spawns_fresh() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        assert_eq!(lc.process_exited("j", 100, 139), vec![Step::ActivateLumen]);
        assert_eq!(lc.select("j", 500), vec![Step::Spawn("j".into())]);
        assert_eq!(
            lc.snapshot().apps,
            vec![AppSnapshot { id: "j".into(), pid: None, windows: vec![], launching: true }],
            "no stale pid or window carried over"
        );
        // the old window id (50) closing late must not end the new run
        assert_eq!(lc.window_closed(50, 0), vec![]);
        assert!(lc.is_running("j"));
    }

    #[test]
    fn exit_after_the_window_closed_is_a_no_op() {
        let mut lc = Lifecycle::new(vec![keep("j")]);
        up(&mut lc, "j");
        lc.window_closed(50, 0);
        assert_eq!(lc.process_exited("j", 100, 0), vec![]);
    }

    // process control
    fn entry(toml: &str) -> Entry {
        crate::registry::parse("t.toml", toml).unwrap()
    }

    #[test]
    fn command_line_for_command_url_and_fallback() {
        let browser = "br --kiosk {url}";
        let cmd = entry("id=\"a\"\nname=\"A\"\nlaunch=\"foo --x  1\"\nfallback_url=\"http://h/it's\"\n");
        assert_eq!(command_line(&cmd, browser, &|p| p == "foo").unwrap(), "foo --x  1");
        assert_eq!(command_line(&cmd, browser, &|_| false).unwrap(), "br --kiosk 'http://h/it'\\''s'");
        let no_fb = entry("id=\"a\"\nname=\"A\"\nlaunch=\"foo\"\n");
        assert!(command_line(&no_fb, browser, &|_| false).unwrap_err().contains("not found"));
        let url = entry("id=\"a\"\nname=\"A\"\ntype=\"url\"\nurl=\"http://x/web\"\n");
        assert_eq!(command_line(&url, browser, &|_| false).unwrap(), "br --kiosk 'http://x/web'");
        assert_eq!(command_line(&url, "br", &|_| false).unwrap(), "br 'http://x/web'");
        assert_eq!(command_line(&url, "br --class=lumen-{id} {url}", &|_| false).unwrap(), "br --class=lumen-a 'http://x/web'");
    }

    #[test]
    fn parse_stat_handles_odd_comms() {
        let p = parse_stat("1234 (kodi.bin) S 1200 1234 1234 0 -1 4194560 ...").unwrap();
        assert_eq!(p, Proc { pid: 1234, ppid: 1200, comm: "kodi.bin".into() });
        let q = parse_stat("77 (Web Content (x)) R 5 77 77 0").unwrap();
        assert_eq!((q.pid, q.ppid, q.comm.as_str()), (77, 5, "Web Content (x)"));
        assert!(parse_stat("garbage").is_none());
        assert!(parse_stat("12 (x) S").is_none());
    }

    fn p(pid: u32, ppid: u32) -> Proc {
        Proc { pid, ppid, comm: "x".into() }
    }

    #[test]
    fn descendants_follow_ppid_links() {
        let table = vec![p(1, 0), p(10, 1), p(11, 10), p(12, 11), p(13, 10), p(20, 1), p(21, 20)];
        let mut d = descendants(10, &table);
        d.sort();
        assert_eq!(d, vec![10, 11, 12, 13]);
        assert_eq!(descendants(99, &table), vec![99], "a vanished root still comes back alone");
        let cyc = vec![p(5, 6), p(6, 5)];
        assert_eq!(descendants(5, &cyc).len(), 2);
    }

    #[test]
    fn process_matching_by_path_or_comm() {
        let path = "/home/laith/.local/share/SteamLink/bin/shell";
        assert!(matches_process(path, Some(path), "shell"));
        assert!(!matches_process(path, Some("/usr/bin/shell"), "shell"));
        assert!(!matches_process(path, None, "shell"));
        assert!(matches_process("chromium", None, "chromium"));
        assert!(!matches_process("chromium", None, "chromium-x"));
        assert!(matches_process("a-very-long-program-name", None, "a-very-long-pro"));
        assert_eq!(expand_tilde("~/x/y", "/home/l"), "/home/l/x/y");
        assert_eq!(expand_tilde("~", "/home/l"), "/home/l");
        assert_eq!(expand_tilde("/a/~b", "/home/l"), "/a/~b");
    }

    #[test]
    fn spawn_runs_and_reaps() {
        let e = entry("id=\"t\"\nname=\"T\"\nlaunch=\"sh -c 'exit 7'\"\n");
        let (tx, rx) = std::sync::mpsc::channel();
        let pid = spawn(&e, DEFAULT_BROWSER, move |id, pid, code| tx.send((id, pid, code)).unwrap()).unwrap();
        let (id, rpid, code) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((id.as_str(), rpid, code), ("t", pid, 7));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn terminate_kills_a_stubborn_child() {
        let e = entry("id=\"t\"\nname=\"T\"\nlaunch=\"sleep 60\"\n");
        let (tx, rx) = std::sync::mpsc::channel();
        let pid = spawn(&e, DEFAULT_BROWSER, move |_, _, code| tx.send(code).unwrap()).unwrap();
        terminate(collect_targets(Some(pid), None), Duration::from_millis(50));
        assert_eq!(rx.recv_timeout(Duration::from_secs(5)).unwrap(), -libc::SIGTERM);
    }
}
