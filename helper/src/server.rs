//! Socket server: one request per connection, one install/remove job at a time.

use crate::allowlist;
use crate::apt::{self, Kind};
use crate::pin::{self, Attempts};
use crate::proto::{self, Checked, Op};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, TryLockError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub struct Config {
    pub socket: Option<PathBuf>,
    pub state_dir: PathBuf,
    pub allowlist: PathBuf,
    pub apt: PathBuf,
    pub idle: Duration,
    pub test_mode: bool,
}

impl Config {
    pub fn pin_file(&self) -> PathBuf {
        self.state_dir.join("pin")
    }
    pub fn attempts_file(&self) -> PathBuf {
        self.state_dir.join("attempts.json")
    }
}

pub struct Server {
    cfg: Config,
    job: Mutex<()>,
    pin_lock: Mutex<()>,
    apt_timeout: Duration,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl Server {
    pub fn new(cfg: Config) -> Server {
        Server { cfg, job: Mutex::new(()), pin_lock: Mutex::new(()), apt_timeout: apt::APT_TIMEOUT }
    }

    fn load_allowlist(&self) -> Result<allowlist::Allowlist, String> {
        allowlist::load(&self.cfg.allowlist, self.cfg.test_mode)
    }

    /// Handles one validated request; returns the final line.
    fn process(&self, req: Checked, emit: &mut dyn FnMut(Value)) -> Value {
        let al = match self.load_allowlist() {
            Ok(a) => a,
            Err(e) => return proto::not_allowed(&e),
        };
        let (op, id, pin) = match req {
            Checked::Status => return self.status(&al),
            Checked::Change { op, id, pin } => (op, id, pin),
        };
        let Some(entry) = al.get(&id) else { return proto::not_allowed("app id is not on the allowlist") };
        let stored = match std::fs::read_to_string(self.cfg.pin_file()) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return proto::no_pin(),
            Err(e) => return proto::error(&format!("pin file: {e}")),
        };
        if let Some(reply) = self.check_pin(&pin, &stored) {
            return reply;
        }
        let _job = match self.job.try_lock() {
            Ok(g) => g,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => return proto::busy(),
        };
        let kind = if op == Op::Install { Kind::Install } else { Kind::Remove };
        apt::run(&self.cfg.apt.to_string_lossy(), kind, entry, self.apt_timeout, emit)
    }

    fn status(&self, al: &allowlist::Allowlist) -> Value {
        {
            let locked_for_s = match Attempts::load(&self.cfg.attempts_file(), now()) {
                Ok(a) => a.locked_for(now()),
                Err(e) => return proto::error(&e),
            };
            let sets: serde_json::Map<String, Value> = al
                .iter()
                .map(|(id, e)| (id.clone(), json!({ "install": e.install, "remove": e.remove })))
                .collect();
            let pin_set = self.cfg.pin_file().exists();
            json!({ "status": { "pin_set": pin_set, "locked_for_s": locked_for_s, "sets": sets } })
        }
    }

    /// None = PIN accepted. Serialised, so parallel guesses can't outrun the counter.
    fn check_pin(&self, pin: &str, stored: &str) -> Option<Value> {
        let _g = self.pin_lock.lock().unwrap_or_else(|p| p.into_inner());
        let path = self.cfg.attempts_file();
        let mut att = match Attempts::load(&path, now()) {
            Ok(a) => a,
            Err(e) => return Some(proto::error(&e)),
        };
        let t = now();
        if att.locked_for(t) > 0 {
            return Some(proto::locked(att.locked_for(t)));
        }
        if pin::verify_pin(pin, stored) {
            if att != Attempts::default() {
                att.reset();
                if let Err(e) = att.save(&path) {
                    return Some(proto::error(&e));
                }
            }
            return None;
        }
        att.record_failure(t);
        if let Err(e) = att.save(&path) {
            return Some(proto::error(&e)); // persist before replying
        }
        Some(proto::bad_pin(att.tries_left(), att.locked_for(t)))
    }

    fn handle(&self, stream: UnixStream) {
        let peer = peer_desc(&stream);
        let _ = stream.set_nonblocking(false);
        let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
        // A client that stops reading must not wedge us: a timed-out write marks the stream broken
        // (further output is dropped) while apt runs to completion.
        let _ = stream.set_write_timeout(Some(Duration::from_secs(10)));
        let mut buf = Vec::new();
        let read = BufReader::new((&stream).take(proto::MAX_REQUEST as u64 + 1)).read_until(b'\n', &mut buf);
        let too_long = matches!(read, Ok(_) if buf.len() > proto::MAX_REQUEST);
        let parsed = match read {
            Err(_) => Err("could not read request".to_string()),
            Ok(_) if too_long => Err("request too long".to_string()),
            Ok(_) if buf.last() != Some(&b'\n') => Err("request not terminated".to_string()),
            Ok(_) => std::str::from_utf8(&buf).map_err(|_| "request is not UTF-8".to_string()).and_then(proto::parse_request),
        };
        let mut broken = false;
        let mut out = &stream;
        let mut send = |v: &Value| {
            if !broken {
                broken = writeln!(out, "{v}").and_then(|_| out.flush()).is_err();
            }
        };
        let (what, last) = match parsed {
            Err(m) => ("-".to_string(), proto::error(&m)),
            Ok(req) => {
                let what = match &req {
                    Checked::Status => "Status".to_string(),
                    Checked::Change { op, id, .. } => format!("{op:?} {id:?}"),
                };
                let last = self.process(req, &mut |v| send(&v));
                (what, last)
            }
        };
        send(&last);
        if too_long {
            // Closing with unread input makes Linux send RST, which can discard the error reply
            // before the client reads it. Half-close, then drain (bounded) until the client closes.
            let _ = stream.shutdown(std::net::Shutdown::Write);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let _ = std::io::copy(&mut (&stream).take(1 << 20), &mut std::io::sink());
        }
        let outcome = last.as_object().and_then(|o| o.keys().next().cloned()).unwrap_or_default();
        eprintln!("lumen-helper: {peer} {what} -> {outcome}");
    }

    /// Accept loop; returns after `idle` with no connection and no job running.
    pub fn serve(self: Arc<Self>, listener: UnixListener) -> std::io::Result<()> {
        listener.set_nonblocking(true)?;
        let active = Arc::new(AtomicUsize::new(0));
        let mut last = Instant::now();
        loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    last = Instant::now();
                    active.fetch_add(1, Ordering::SeqCst);
                    let (me, active) = (self.clone(), active.clone());
                    std::thread::spawn(move || {
                        me.handle(stream);
                        active.fetch_sub(1, Ordering::SeqCst);
                    });
                }
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    if active.load(Ordering::SeqCst) > 0 {
                        last = Instant::now();
                    } else if last.elapsed() >= self.cfg.idle {
                        eprintln!("lumen-helper: idle, exiting");
                        return Ok(());
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
                Err(e) => return Err(e),
            }
        }
    }
}

#[cfg(target_os = "linux")]
fn peer_desc(s: &UnixStream) -> String {
    use std::os::fd::AsRawFd;
    let mut cred = libc::ucred { pid: 0, uid: 0, gid: 0 };
    let mut len = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let r = unsafe { libc::getsockopt(s.as_raw_fd(), libc::SOL_SOCKET, libc::SO_PEERCRED, &mut cred as *mut _ as *mut libc::c_void, &mut len) };
    if r == 0 {
        format!("uid={} pid={}", cred.uid, cred.pid)
    } else {
        "uid=? pid=?".into()
    }
}

#[cfg(not(target_os = "linux"))]
fn peer_desc(_: &UnixStream) -> String {
    "uid=? pid=?".into()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lh-srv-{name}-{}", std::process::id()));
        std::fs::remove_dir_all(&d).ok();
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn server(dir: &PathBuf) -> Server {
        std::fs::write(dir.join("al.toml"), "[a]\ninstall=[\"pkg1\"]\n").unwrap();
        Server::new(Config {
            socket: None,
            state_dir: dir.clone(),
            allowlist: dir.join("al.toml"),
            apt: "/nonexistent".into(),
            idle: Duration::from_secs(1),
            test_mode: true,
        })
    }

    #[test]
    fn wrong_pin_counts_and_locks_without_checking() {
        let dir = tmp("lock");
        let s = server(&dir);
        let stored = pin::hash_pin("1234").unwrap();
        for left in (1..=4).rev() {
            assert_eq!(s.check_pin("0000", &stored), Some(proto::bad_pin(Some(left), 0)));
        }
        let reply = s.check_pin("0000", &stored).unwrap();
        assert_eq!(reply["bad_pin"]["tries_left"], Value::Null);
        assert!(matches!(reply["bad_pin"]["locked_for_s"].as_u64(), Some(55..=60)), "{reply}");
        // Locked: even the right PIN is refused, and the state survived on disk.
        let reply = s.check_pin("1234", &stored).unwrap();
        assert!(matches!(reply["locked"]["locked_for_s"].as_u64(), Some(55..=60)), "{reply}");
        assert_eq!(Attempts::load(&s.cfg.attempts_file(), now()).unwrap().failures, 5);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn right_pin_resets_counter() {
        let dir = tmp("reset");
        let s = server(&dir);
        let stored = pin::hash_pin("1234").unwrap();
        s.check_pin("0000", &stored);
        assert_eq!(s.check_pin("1234", &stored), None);
        assert_eq!(Attempts::load(&s.cfg.attempts_file(), now()).unwrap(), Attempts::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    fn change(op: Op, id: &str, pin: &str) -> Checked {
        Checked::Change { op, id: id.into(), pin: pin.into() }
    }

    fn set_pin(dir: &PathBuf, pin: &str) -> String {
        let stored = pin::hash_pin(pin).unwrap();
        std::fs::write(dir.join("pin"), &stored).unwrap();
        stored
    }

    #[test]
    fn unknown_id_is_refused_before_the_pin_is_looked_at() {
        let dir = tmp("unknown-id");
        let s = server(&dir);
        set_pin(&dir, "1234");
        let r = s.process(change(Op::Install, "nope", "0000"), &mut |_| {});
        assert!(r.get("not_allowed").is_some(), "{r}");
        assert!(!s.cfg.attempts_file().exists(), "a guess at an unlisted id must not touch the counter");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn no_pin_file_means_no_pin_and_no_counter() {
        let dir = tmp("no-pin");
        let s = server(&dir);
        assert_eq!(s.process(change(Op::Install, "a", "1234"), &mut |_| {}), proto::no_pin());
        assert_eq!(s.process(change(Op::Remove, "a", "1234"), &mut |_| {}), proto::no_pin());
        assert!(!s.cfg.attempts_file().exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_pin_file_never_accepts_a_pin() {
        let dir = tmp("bad-hash");
        let s = server(&dir);
        for junk in ["", "\n", "1234", "$argon2id$garbage", "plain"] {
            std::fs::write(dir.join("pin"), junk).unwrap();
            std::fs::remove_file(s.cfg.attempts_file()).ok();
            let r = s.process(change(Op::Install, "a", "1234"), &mut |_| {});
            assert!(r.get("bad_pin").is_some(), "{junk:?} -> {r}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_attempts_file_fails_closed_for_jobs_and_status() {
        let dir = tmp("bad-attempts");
        let s = server(&dir);
        set_pin(&dir, "1234");
        std::fs::write(s.cfg.attempts_file(), "garbage").unwrap();
        let mut ran = false;
        let r = s.process(change(Op::Install, "a", "1234"), &mut |_| ran = true);
        assert!(r.get("error").is_some(), "right PIN must not get through without readable counter state: {r}");
        assert!(!ran);
        assert!(s.process(Checked::Status, &mut |_| {}).get("error").is_some());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn right_pin_reaches_apt_and_wrong_pin_does_not() {
        let dir = tmp("reach-apt");
        let s = server(&dir); // apt = /nonexistent, so reaching it shows up as a failed start
        set_pin(&dir, "1234");
        let r = s.process(change(Op::Install, "a", "1234"), &mut |_| {});
        assert!(r["failed"]["message"].as_str().unwrap().contains("cannot start apt-get"), "{r}");
        let r = s.process(change(Op::Install, "a", "4321"), &mut |_| {});
        assert!(r.get("bad_pin").is_some(), "{r}");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_expired_lock_lets_the_right_pin_in_and_a_wrong_one_locks_longer() {
        let dir = tmp("expired");
        let s = server(&dir);
        let stored = set_pin(&dir, "1234");
        let past = Attempts { failures: 5, locked_until: now().saturating_sub(1) };
        past.save(&s.cfg.attempts_file()).unwrap();
        let r = s.check_pin("0000", &stored).unwrap();
        assert_eq!(r["bad_pin"]["tries_left"], Value::Null);
        assert!(matches!(r["bad_pin"]["locked_for_s"].as_u64(), Some(115..=120)), "second lock is 120 s: {r}");
        assert_eq!(Attempts::load(&s.cfg.attempts_file(), now()).unwrap().failures, 6);

        past.save(&s.cfg.attempts_file()).unwrap();
        assert_eq!(s.check_pin("1234", &stored), None);
        assert_eq!(Attempts::load(&s.cfg.attempts_file(), now()).unwrap(), Attempts::default());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn status_reports_pin_set_lock_and_sets_but_never_the_hash() {
        let dir = tmp("status");
        let s = server(&dir);
        let r = s.process(Checked::Status, &mut |_| {});
        assert_eq!(r["status"]["pin_set"], false);
        let stored = set_pin(&dir, "1234");
        Attempts { failures: 5, locked_until: now() + 50 }.save(&s.cfg.attempts_file()).unwrap();
        let r = s.process(Checked::Status, &mut |_| {});
        assert_eq!(r["status"]["pin_set"], true);
        assert!(matches!(r["status"]["locked_for_s"].as_u64(), Some(45..=50)), "{r}");
        assert_eq!(r["status"]["sets"]["a"], json!({"install": ["pkg1"], "remove": []}));
        assert!(!r.to_string().contains(stored.trim()));
        assert!(!r.to_string().contains("argon2"));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn an_unsafe_allowlist_stops_jobs_before_any_pin_check() {
        let dir = tmp("bad-list");
        let s = server(&dir);
        set_pin(&dir, "1234");
        std::fs::write(dir.join("al.toml"), "[a]\ninstall=[\"-y\"]\n").unwrap();
        assert!(s.process(change(Op::Install, "a", "0000"), &mut |_| {}).get("not_allowed").is_some());
        assert!(!s.cfg.attempts_file().exists());
        std::fs::write(dir.join("al.toml"), "").unwrap();
        assert!(s.process(change(Op::Install, "a", "1234"), &mut |_| {}).get("not_allowed").is_some(), "empty list allows nothing");
        std::fs::remove_dir_all(&dir).ok();
    }
}
