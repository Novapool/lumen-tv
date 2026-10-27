//! Runs apt-get with an argument vector only: no shell, cleared environment.

use crate::allowlist::Entry;
use crate::proto;
use serde_json::Value;
use std::io::Read;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

pub const APT_TIMEOUT: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Install,
    Remove,
}

const COMMON: [&str; 7] = [
    "-y",
    "-o",
    "DPkg::Lock::Timeout=120",
    "-o",
    "Dpkg::Options::=--force-confdef",
    "-o",
    "Dpkg::Options::=--force-confold",
];

pub fn update_args() -> Vec<String> {
    COMMON.iter().chain(["update"].iter()).map(|s| s.to_string()).collect()
}

/// `COMMON [-s] [--no-install-recommends --no-remove] install|remove -- <names>`
/// (`-s` = simulate: nothing is changed, apt prints `Inst`/`Remv` lines).
pub fn args(kind: Kind, names: &[String], simulate: bool) -> Vec<String> {
    let mut a: Vec<String> = COMMON.iter().map(|s| s.to_string()).collect();
    if simulate {
        a.push("-s".into());
    }
    if kind == Kind::Install {
        a.push("--no-install-recommends".into());
        a.push("--no-remove".into());
    }
    a.push(if kind == Kind::Install { "install" } else { "remove" }.into());
    a.push("--".into());
    a.extend(names.iter().cloned());
    a
}

/// Package name from an `Inst`/`Remv` simulation line: second word, without `:arch`.
fn sim_pkg(line: &str, tag: &str) -> Option<String> {
    let rest = line.strip_prefix(tag)?.trim_start();
    let name = rest.split_whitespace().next()?;
    Some(name.split(':').next().unwrap_or(name).to_string())
}

/// The simulation must not do more than the allowlist says: installs may not remove anything,
/// removes may not install anything or remove packages other than the entry's `remove` list.
pub fn check_simulation(kind: Kind, lines: &[String], allowed_remove: &[String]) -> Result<(), String> {
    let allowed: Vec<&str> = allowed_remove.iter().map(|n| n.split(':').next().unwrap_or(n)).collect();
    let mut extra_remove = Vec::new();
    let mut extra_install = Vec::new();
    for l in lines {
        if let Some(p) = sim_pkg(l, "Remv ") {
            if kind == Kind::Install || !allowed.contains(&p.as_str()) {
                extra_remove.push(p);
            }
        } else if let Some(p) = sim_pkg(l, "Inst ") {
            if kind == Kind::Remove {
                extra_install.push(p);
            }
        }
    }
    let mut why = Vec::new();
    if !extra_remove.is_empty() {
        why.push(format!("would also remove: {}", extra_remove.join(", ")));
    }
    if !extra_install.is_empty() {
        why.push(format!("would install: {}", extra_install.join(", ")));
    }
    if why.is_empty() {
        Ok(())
    } else {
        Err(format!("refused, not on the allowlist: {}", why.join("; ")))
    }
}

fn command(apt: &str, args: &[String]) -> Command {
    let mut cmd = Command::new(apt);
    cmd.args(args)
        .env_clear()
        .env("PATH", "/usr/sbin:/usr/bin:/sbin:/bin")
        .env("DEBIAN_FRONTEND", "noninteractive")
        .env("LC_ALL", "C.UTF-8")
        .stdin(Stdio::null());
    cmd
}

/// Splits a byte stream into lines on `\n` and `\r`; a segment after a `\r` replaces the last line.
#[derive(Default)]
pub struct Splitter {
    buf: Vec<u8>,
    after_cr: bool,
}

impl Splitter {
    fn emit(&mut self, out: &mut Vec<Value>, term: Option<u8>) {
        let text = String::from_utf8_lossy(&self.buf).trim_end().to_string();
        self.buf.clear();
        if text.is_empty() {
            if term == Some(b'\n') {
                self.after_cr = false;
            }
            return;
        }
        out.push(if self.after_cr { proto::replace(&text) } else { proto::line(&text) });
        self.after_cr = term == Some(b'\r');
    }
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<Value> {
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
    pub fn finish(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        self.emit(&mut out, None);
        out
    }
}

fn text_of(v: &Value) -> Option<&str> {
    v.get("line").or_else(|| v.get("replace")).and_then(Value::as_str)
}

/// Runs one apt-get invocation, streaming events. Err((code, message)) on failure.
fn run_one(apt: &str, args: &[String], timeout: Duration, emit: &mut dyn FnMut(Value)) -> Result<(), (i32, String)> {
    emit(proto::step(&format!("apt-get {}", args.join(" "))));
    let fail = |m: String| (-1, m);
    let (reader, writer) = std::io::pipe().map_err(|e| fail(e.to_string()))?;
    let mut cmd = command(apt, args);
    cmd.stdout(writer.try_clone().map_err(|e| fail(e.to_string()))?).stderr(writer);
    std::os::unix::process::CommandExt::process_group(&mut cmd, 0);
    let mut child = cmd.spawn().map_err(|e| fail(format!("cannot start apt-get: {e}")))?;
    drop(cmd); // close our copies of the write end so EOF can arrive
    let pgid = child.id() as i32;
    let kill_group = || unsafe {
        libc::kill(-pgid, libc::SIGKILL);
    };

    let (tx, rx) = mpsc::channel::<Value>();
    std::thread::spawn(move || {
        let mut reader = reader;
        let mut split = Splitter::default();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(n) => split.feed(&chunk[..n]).into_iter().for_each(|v| {
                    let _ = tx.send(v);
                }),
            }
        }
        split.finish().into_iter().for_each(|v| {
            let _ = tx.send(v);
        });
    });

    let started = Instant::now();
    let mut last = String::new();
    let mut timed_out = false;
    let mut exited_at: Option<Instant> = None;
    loop {
        match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(v) => {
                if let Some(t) = text_of(&v) {
                    last = t.to_string();
                }
                emit(v);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if exited_at.is_none() && matches!(child.try_wait(), Ok(Some(_))) {
            exited_at = Some(Instant::now());
        }
        // apt-get is gone but something still holds the pipe: give output 2 s, then stop reading.
        if exited_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(2)) {
            kill_group();
            break;
        }
        if started.elapsed() >= timeout {
            timed_out = true;
            kill_group();
            break;
        }
    }
    let status = child.wait().map_err(|e| fail(e.to_string()))?;
    if timed_out {
        return Err((-1, "apt-get timed out".into()));
    }
    if status.success() {
        return Ok(());
    }
    let code = status.code().unwrap_or(-1);
    let msg = if last.is_empty() { format!("apt-get exited with code {code}") } else { last };
    Err((code, msg))
}

/// Runs the allowlisted work for one app. An empty `remove` list runs nothing and succeeds.
pub fn run(apt: &str, kind: Kind, entry: &Entry, timeout: Duration, emit: &mut dyn FnMut(Value)) -> Value {
    let names = if kind == Kind::Install { &entry.install } else { &entry.remove };
    if names.is_empty() {
        return proto::done();
    }
    if kind == Kind::Install && entry.update {
        if let Err((code, msg)) = run_one(apt, &update_args(), timeout, emit) {
            return proto::failed(code, &msg);
        }
    }
    // Simulate first: show the step, keep the output, and refuse if apt would do more than allowed.
    let mut sim = Vec::new();
    let res = run_one(apt, &args(kind, names, true), timeout, &mut |v| {
        if v.get("step").is_some() {
            emit(v);
        } else if let Some(t) = text_of(&v) {
            sim.push(t.to_string());
        }
    });
    if let Err((code, msg)) = res {
        return proto::failed(code, &msg);
    }
    if let Err(msg) = check_simulation(kind, &sim, &entry.remove) {
        return proto::failed(2, &msg);
    }
    match run_one(apt, &args(kind, names, false), timeout, emit) {
        Ok(()) => proto::done(),
        Err((code, msg)) => proto::failed(code, &msg),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: [&str; 7] = ["-y", "-o", "DPkg::Lock::Timeout=120", "-o", "Dpkg::Options::=--force-confdef", "-o", "Dpkg::Options::=--force-confold"];

    fn v(parts: &[&[&str]]) -> Vec<String> {
        parts.iter().flat_map(|p| p.iter().map(|s| s.to_string())).collect()
    }

    #[test]
    fn install_argv_shape() {
        let a = args(Kind::Install, &["sl".into(), "kodi".into()], false);
        assert_eq!(a, v(&[&CONF, &["--no-install-recommends", "--no-remove", "install", "--", "sl", "kodi"]]));
        let s = args(Kind::Install, &["sl".into()], true);
        assert_eq!(s, v(&[&CONF, &["-s", "--no-install-recommends", "--no-remove", "install", "--", "sl"]]));
    }

    #[test]
    fn remove_argv_shape() {
        let a = args(Kind::Remove, &["kodi".into()], false);
        assert_eq!(a, v(&[&CONF, &["remove", "--", "kodi"]]));
        assert_eq!(args(Kind::Remove, &["kodi".into()], true), v(&[&CONF, &["-s", "remove", "--", "kodi"]]));
        assert!(!a.iter().any(|x| x == "autoremove" || x == "purge"));
    }

    #[test]
    fn names_always_after_double_dash() {
        let a = args(Kind::Install, &["-oEvil=1".into()], false);
        let dd = a.iter().position(|x| x == "--").unwrap();
        assert_eq!(a.iter().position(|x| x == "-oEvil=1").unwrap(), dd + 1);
    }

    #[test]
    fn update_argv_shape() {
        assert_eq!(update_args(), v(&[&CONF, &["update"]]));
    }

    #[test]
    fn no_shell_is_involved() {
        let c = command("/usr/bin/apt-get", &args(Kind::Install, &["sl".into()], false));
        assert_eq!(c.get_program(), "/usr/bin/apt-get");
        let envs: Vec<_> = c.get_envs().map(|(k, _)| k.to_string_lossy().into_owned()).collect();
        assert_eq!(envs.len(), 3);
    }

    fn lines(l: &[&str]) -> Vec<String> {
        l.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn simulation_install_rules() {
        let ok = lines(&["Reading package lists...", "Inst sl (5.02 Debian:stable [arm64])", "Inst libx:arm64 (1 x)", "Conf sl (5.02)"]);
        assert!(check_simulation(Kind::Install, &ok, &[]).is_ok());
        let bad = lines(&["Inst sl (1)", "Remv vlc [3.0]"]);
        let e = check_simulation(Kind::Install, &bad, &["vlc".into()]).unwrap_err();
        assert!(e.contains("would also remove: vlc"), "{e}");
    }

    #[test]
    fn simulation_remove_rules() {
        let allowed = vec!["kodi".to_string(), "kodi-bin:arm64".to_string()];
        let ok = lines(&["Remv kodi [21.1] [kodi-bin:arm64 ]", "Remv kodi-bin:arm64 [21.1]", "Remv kodi:arm64 [1]"]);
        assert!(check_simulation(Kind::Remove, &ok, &allowed).is_ok());
        let extra = lines(&["Remv kodi [1]", "Remv libfoo:arm64 [2]", "Remv ubuntu-desktop [3]"]);
        let e = check_simulation(Kind::Remove, &extra, &allowed).unwrap_err();
        assert!(e.contains("libfoo, ubuntu-desktop"), "{e}");
        let inst = lines(&["Remv kodi [1]", "Inst surprise (1)"]);
        assert!(check_simulation(Kind::Remove, &inst, &allowed).unwrap_err().contains("would install: surprise"));
    }

    #[test]
    fn splitter_cr_lf() {
        let mut s = Splitter::default();
        let v = s.feed(b"a\nP 10%\rP 50%\r\nend\nlast");
        let texts: Vec<_> = v.iter().map(|x| x.to_string()).collect();
        assert_eq!(texts, [r#"{"line":"a"}"#, r#"{"line":"P 10%"}"#, r#"{"replace":"P 50%"}"#, r#"{"line":"end"}"#]);
        assert_eq!(s.finish()[0].to_string(), r#"{"line":"last"}"#);
    }

    #[test]
    fn empty_remove_runs_nothing() {
        let e = Entry { install: vec!["x1".into()], remove: vec![], update: true };
        let mut n = 0;
        let r = run("/nonexistent", Kind::Remove, &e, APT_TIMEOUT, &mut |_| n += 1);
        assert_eq!(r, proto::done());
        assert_eq!(n, 0);
    }

    #[test]
    fn simulation_matches_whole_package_names_only() {
        // "kodi" being removable says nothing about packages that merely start with it.
        let allowed = vec!["kodi".to_string()];
        for l in ["Remv kodi-extra [1]", "Remv kodi2 [1]", "Remv kodi-bin:arm64 [1]", "Remv xkodi [1]"] {
            let e = check_simulation(Kind::Remove, &lines(&[l]), &allowed).unwrap_err();
            assert!(e.contains("would also remove"), "{l}: {e}");
        }
        assert!(check_simulation(Kind::Remove, &lines(&["Remv  kodi  [1]"]), &allowed).is_ok(), "extra spaces");
        assert!(check_simulation(Kind::Remove, &lines(&["Remv kodi:arm64 [1]"]), &allowed).is_ok());
    }

    #[test]
    fn install_refuses_every_removal_even_of_a_listed_package() {
        // An install may never take anything out, whatever the entry's remove list says.
        let l = lines(&["Inst sl (1)", "Remv kodi:arm64 [1]"]);
        assert!(check_simulation(Kind::Install, &l, &["kodi".into()]).is_err());
    }

    #[test]
    fn simulation_ignores_noise_and_unrelated_lines() {
        let l = lines(&["", "Remvfoo [1]", "NOTE: Remv x", "Conf kodi (1)", "Removing kodi ...", "Inst", "Remv "]);
        assert!(check_simulation(Kind::Remove, &l, &[]).is_ok());
        assert!(check_simulation(Kind::Install, &l, &[]).is_ok());
    }

    #[test]
    fn simulation_reports_all_extra_changes_together() {
        let l = lines(&["Remv a1 [1]", "Inst b2 (1)", "Remv c3 [1]"]);
        let e = check_simulation(Kind::Remove, &l, &[]).unwrap_err();
        assert_eq!(e, "refused, not on the allowlist: would also remove: a1, c3; would install: b2");
    }

    #[test]
    fn splitter_handles_chunk_boundaries_crlf_blanks_and_bad_utf8() {
        let mut s = Splitter::default();
        assert!(s.feed(b"hel").is_empty());
        assert_eq!(s.feed(b"lo\r").len(), 1);
        // CR then LF is one line break, not a replace followed by an empty line.
        let v = s.feed(b"\nnext\r\n\r\n\n  \nz\n");
        let t: Vec<_> = v.iter().map(|x| x.to_string()).collect();
        assert_eq!(t, [r#"{"line":"next"}"#, r#"{"line":"z"}"#]);
        let v = s.feed(b"a\xffb \n");
        assert_eq!(v[0]["line"], "a\u{fffd}b");
        assert!(s.finish().is_empty());
    }

    #[test]
    fn a_progress_line_after_a_blank_cr_still_replaces() {
        let mut s = Splitter::default();
        let v = s.feed(b"P 1%\r\rP 2%\n");
        let t: Vec<_> = v.iter().map(|x| x.to_string()).collect();
        assert_eq!(t, [r#"{"line":"P 1%"}"#, r#"{"replace":"P 2%"}"#]);
    }

    #[test]
    fn argv_never_carries_shell_or_option_text_from_anywhere_but_the_names() {
        for kind in [Kind::Install, Kind::Remove] {
            for sim in [false, true] {
                let a = args(kind, &["pkg1".into()], sim);
                let dd = a.iter().position(|x| x == "--").unwrap();
                assert_eq!(&a[dd + 1..], ["pkg1"]);
                assert!(a[..dd].iter().all(|x| !x.contains(' ') || x.starts_with("DPkg") || x.starts_with("Dpkg")), "{a:?}");
                assert!(!a.iter().any(|x| x == "sh" || x == "-c" || x == "--purge" || x == "autoremove"));
            }
        }
    }
}
