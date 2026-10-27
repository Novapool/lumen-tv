//! Drives `lumen-helper serve --insecure-test-mode` over a real socket with a fake apt-get.

use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_lumen-helper");

struct Harness {
    dir: PathBuf,
    child: Option<Child>,
}

impl Harness {
    fn sock(&self) -> PathBuf {
        self.dir.join("h.sock")
    }
    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.join("apt.log")).unwrap_or_default()
    }
    fn args(&self, cmd: &str) -> Command {
        let mut c = Command::new(BIN);
        c.arg(cmd)
            .args(["--insecure-test-mode", "--socket"])
            .arg(self.sock())
            .arg("--state-dir")
            .arg(self.dir.join("state"))
            .arg("--allowlist")
            .arg(self.dir.join("allow.toml"))
            .arg("--apt")
            .arg(self.dir.join("fake-apt.sh"));
        c
    }
    fn set_pin(&self, pin: &str) {
        let mut c = self.args("set-pin").stdin(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
        write!(c.stdin.take().unwrap(), "{pin}\n{pin}\n").unwrap();
        assert!(c.wait().unwrap().success());
    }
    fn start(&mut self, idle: &str) {
        let c = self.args("serve").arg("--idle-exit").arg(idle).stderr(Stdio::null()).spawn().unwrap();
        self.child = Some(c);
        let t = Instant::now();
        while !self.sock().exists() {
            assert!(t.elapsed() < Duration::from_secs(10), "socket never appeared");
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
        std::fs::remove_dir_all(&self.dir).ok();
    }
}

fn harness(name: &str) -> Harness {
    let dir = std::env::temp_dir().join(format!("lh-it-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/fake-apt.sh");
    std::fs::copy(root, dir.join("fake-apt.sh")).unwrap();
    std::fs::write(
        dir.join("allow.toml"),
        "[root-test]\ninstall=[\"sl\"]\nremove=[\"sl\"]\n\n[kodi]\ninstall=[\"kodi\",\"kodi-x\"]\nremove=[\"kodi\"]\nupdate=false\n\n\
         [slow]\ninstall=[\"slowpkg\"]\nupdate=false\n\n[bad]\ninstall=[\"failpkg\"]\nupdate=false\n\n[noremove]\ninstall=[\"keepme\"]\nupdate=false\n",
    )
    .unwrap();
    Harness { dir, child: None }
}

/// Sends one request line, returns every reply line.
fn ask(h: &Harness, req: Value) -> Vec<Value> {
    let mut s = UnixStream::connect(h.sock()).unwrap();
    writeln!(s, "{req}").unwrap();
    let mut out = Vec::new();
    for l in BufReader::new(s).lines() {
        out.push(serde_json::from_str(&l.unwrap()).unwrap());
    }
    out
}

fn last(v: &[Value]) -> &Value {
    v.last().unwrap()
}

fn req(op: &str, id: &str, pin: &str) -> Value {
    json!({"v": 1, "op": op, "id": id, "pin": pin})
}

#[test]
fn status_without_pin_then_no_pin_for_install() {
    let mut h = harness("nopin");
    h.start("30");
    let r = ask(&h, json!({"v":1,"op":"status"}));
    assert_eq!(r.len(), 1);
    let st = &r[0]["status"];
    assert_eq!(st["pin_set"], false);
    assert_eq!(st["locked_for_s"], 0);
    assert_eq!(st["sets"]["kodi"], json!({"install": ["kodi","kodi-x"], "remove": ["kodi"]}));
    assert_eq!(st["sets"]["root-test"]["install"], json!(["sl"]));
    assert_eq!(last(&ask(&h, req("install", "root-test", "1234"))), &json!({"no_pin": true}));
    assert_eq!(h.log(), "", "apt must not run");
}

#[test]
fn pin_flow_install_remove_and_lockout() {
    let mut h = harness("flow");
    h.set_pin("4321");
    assert_eq!(std::fs::read_to_string(h.dir.join("state/pin")).unwrap().trim().starts_with("$argon2id$"), true);
    h.start("30");
    assert_eq!(ask(&h, json!({"v":1,"op":"status"}))[0]["status"]["pin_set"], true);

    // not on the allowlist: refused before the PIN is even looked at, apt untouched
    assert!(last(&ask(&h, req("install", "nope", "4321"))).get("not_allowed").is_some());
    // malformed
    assert!(last(&ask(&h, json!({"v":1,"op":"install","id":"root-test","pin":"4321","extra":1}))).get("error").is_some());
    assert!(last(&ask(&h, req("install", "root-test", "12"))).get("error").is_some());
    assert_eq!(h.log(), "");

    // wrong PIN: counts down
    let r = ask(&h, req("install", "root-test", "0000"));
    assert_eq!(last(&r), &json!({"bad_pin": {"tries_left": 4, "locked_for_s": 0}}));
    assert_eq!(h.log(), "");

    // right PIN: update (root-test defaults to update=true) then install, streamed
    let r = ask(&h, req("install", "root-test", "4321"));
    assert_eq!(last(&r), &json!({"done": true}));
    let steps: Vec<_> = r.iter().filter_map(|v| v["step"].as_str()).collect();
    assert_eq!(
        steps,
        [
            "apt-get -y -o DPkg::Lock::Timeout=120 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold update",
            "apt-get -y -o DPkg::Lock::Timeout=120 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold -s --no-install-recommends --no-remove install -- sl",
            "apt-get -y -o DPkg::Lock::Timeout=120 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold --no-install-recommends --no-remove install -- sl"
        ]
    );
    assert!(r.iter().any(|v| v["line"] == "Reading package lists..."));
    assert!(r.iter().any(|v| v["replace"] == "Progress: 60%"), "{r:?}");
    let log = h.log();
    assert!(log.contains("ARG:--\nARG:sl\n"), "{log}");
    // cleared environment: only what the helper sets (plus what sh itself adds)
    for forbidden in ["ENV:HOME=", "ENV:USER=", "ENV:SSH_", "ENV:TERM="] {
        assert!(!log.contains(forbidden), "{forbidden} leaked into apt env");
    }
    assert!(log.contains("ENV:PATH=/usr/sbin:/usr/bin:/sbin:/bin\n"));
    assert!(log.contains("ENV:DEBIAN_FRONTEND=noninteractive\n"));
    assert!(log.contains("ENV:LC_ALL=C.UTF-8\n"));
    // right PIN reset the counter
    let r = ask(&h, req("install", "root-test", "0000"));
    assert_eq!(last(&r)["bad_pin"]["tries_left"], 4);

    // remove: uses the remove list, no recommends flag, no update
    let before = h.log().len();
    assert_eq!(last(&ask(&h, req("remove", "kodi", "4321"))), &json!({"done": true}));
    let log = h.log()[before..].to_string();
    let conf = "-y -o DPkg::Lock::Timeout=120 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold";
    assert!(log.contains(&format!("ARGS:{conf} -s remove -- kodi\n")), "{log}");
    assert!(log.contains(&format!("ARGS:{conf} remove -- kodi\n")), "{log}");
    assert!(!log.contains("no-install-recommends"));

    // empty remove list runs nothing and succeeds
    let before = h.log().len();
    assert_eq!(last(&ask(&h, req("remove", "noremove", "4321"))), &json!({"done": true}));
    assert_eq!(h.log().len(), before);

    // failure is reported with the last output line
    let r = ask(&h, req("install", "bad", "4321"));
    assert_eq!(last(&r), &json!({"failed": {"code": 100, "message": "E: Unable to locate package failpkg"}}));

    // lockout: 5 wrong in a row (one already failed above and was reset by the success, so 5 fresh)
    for left in (1..=4).rev() {
        assert_eq!(last(&ask(&h, req("install", "root-test", "1111")))["bad_pin"]["tries_left"], left);
    }
    assert_eq!(last(&ask(&h, req("install", "root-test", "1111"))), &json!({"bad_pin": {"tries_left": null, "locked_for_s": 60}}));
    let before = h.log().len();
    let r = ask(&h, req("install", "root-test", "4321"));
    assert_eq!(last(&r)["locked"]["locked_for_s"].as_u64().map(|s| s > 55 && s <= 60), Some(true));
    assert_eq!(h.log().len(), before, "locked: nothing runs");
    assert!(ask(&h, json!({"v":1,"op":"status"}))[0]["status"]["locked_for_s"].as_u64().unwrap() > 55);

    // persisted: restart the daemon and it is still locked
    let mut c = h.child.take().unwrap();
    c.kill().unwrap();
    c.wait().unwrap();
    std::fs::remove_file(h.sock()).ok();
    h.start("30");
    assert!(last(&ask(&h, req("install", "root-test", "4321"))).get("locked").is_some());
}

#[test]
fn simulation_refuses_extra_changes() {
    let mut h = harness("sim");
    h.set_pin("1470");
    h.start("30");
    let extra = h.dir.join("sim.extra");

    // remove would take other packages with it
    std::fs::write(&extra, "Remv ubuntu-desktop [1]\nRemv libfoo:arm64 [2]\n").unwrap();
    let r = ask(&h, req("remove", "kodi", "1470"));
    let f = &last(&r)["failed"];
    assert!(f["message"].as_str().unwrap().contains("ubuntu-desktop, libfoo"), "{r:?}");
    assert!(!h.log().lines().any(|l| l == "ARGS:-y -o DPkg::Lock::Timeout=120 -o Dpkg::Options::=--force-confdef -o Dpkg::Options::=--force-confold remove -- kodi"), "real run must not happen");
    assert!(r.iter().any(|v| v["step"].as_str().is_some_and(|s| s.contains(" -s remove"))), "simulation step shown");

    // remove would install something
    std::fs::write(&extra, "Inst surprise (1)\n").unwrap();
    assert!(last(&ask(&h, req("remove", "kodi", "1470")))["failed"]["message"].as_str().unwrap().contains("would install: surprise"));

    // install would remove something
    std::fs::write(&extra, "Remv vlc [3]\n").unwrap();
    let before = h.log().matches("ARGS:").count();
    let r = ask(&h, req("install", "kodi", "1470"));
    assert!(last(&r)["failed"]["message"].as_str().unwrap().contains("would also remove: vlc"), "{r:?}");
    // only the simulation ran (update=false for kodi)
    assert_eq!(h.log().matches("ARGS:").count(), before + 1);

    // listed removals are fine: "Remv kodi" is in kodi's remove list; clean path completes
    std::fs::write(&extra, "Remv kodi:arm64 [1]\n").unwrap();
    assert_eq!(last(&ask(&h, req("remove", "kodi", "1470"))), &json!({"done": true}));
    std::fs::remove_file(&extra).unwrap();
    assert_eq!(last(&ask(&h, req("install", "kodi", "1470"))), &json!({"done": true}));
}

#[test]
fn path_overrides_need_test_mode() {
    let out = Command::new(BIN).args(["check", "--allowlist", "/tmp/x.toml"]).output().unwrap();
    assert!(!out.status.success());
    assert!(String::from_utf8_lossy(&out.stderr).contains("--insecure-test-mode"));
}

#[test]
fn second_job_gets_busy() {
    let mut h = harness("busy");
    h.set_pin("2468");
    h.start("30");
    let sock = h.sock();
    let t = std::thread::spawn(move || {
        let mut s = UnixStream::connect(sock).unwrap();
        writeln!(s, "{}", req("install", "slow", "2468")).unwrap();
        let mut all = String::new();
        s.read_to_string(&mut all).unwrap();
        all
    });
    // wait until the fake apt has started
    let t0 = Instant::now();
    while !h.log().contains("slowpkg") {
        assert!(t0.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(20));
    }
    assert_eq!(last(&ask(&h, req("install", "root-test", "2468"))), &json!({"busy": true}));
    assert!(t.join().unwrap().contains("\"done\":true"));
    // free again afterwards
    assert_eq!(last(&ask(&h, req("remove", "kodi", "2468"))), &json!({"done": true}));
}

#[test]
fn unsafe_or_invalid_allowlist_is_refused_per_request() {
    let mut h = harness("badlist");
    h.set_pin("1357");
    h.start("30");
    std::fs::write(h.dir.join("allow.toml"), "[x]\ninstall=[\"--force-yes\"]\n").unwrap();
    let r = ask(&h, req("install", "x", "1357"));
    assert!(last(&r).get("not_allowed").is_some());
    assert!(last(&ask(&h, json!({"v":1,"op":"status"}))).get("not_allowed").is_some());
    assert_eq!(h.log(), "");
    // re-read on every request: fix the file and it works with no restart
    std::fs::write(h.dir.join("allow.toml"), "[x]\ninstall=[\"okpkg\"]\nupdate=false\n").unwrap();
    assert_eq!(last(&ask(&h, req("install", "x", "1357"))), &json!({"done": true}));
}

#[test]
fn oversized_and_garbage_requests_get_error() {
    let mut h = harness("junk");
    h.start("30");
    let mut s = UnixStream::connect(h.sock()).unwrap();
    s.write_all(&vec![b'a'; 5000]).unwrap();
    s.write_all(b"\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.contains("\"error\""), "{out}");
    let mut s = UnixStream::connect(h.sock()).unwrap();
    s.write_all(b"not json\n").unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).unwrap();
    assert!(out.contains("\"error\""));
}

#[test]
fn exits_after_idle_timeout() {
    let mut h = harness("idle");
    h.start("1");
    let mut c = h.child.take().unwrap();
    let t = Instant::now();
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            assert!(st.success());
            break;
        }
        assert!(t.elapsed() < Duration::from_secs(10), "did not exit when idle");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn check_prints_sets_and_set_pin_validates() {
    let h = harness("check");
    let out = h.args("check").output().unwrap();
    assert!(out.status.success());
    let text = String::from_utf8_lossy(&out.stdout);
    assert!(text.contains("[kodi]") && text.contains("NOT set"), "{text}");
    std::fs::write(h.dir.join("allow.toml"), "[bad id]\ninstall=[\"sl\"]\n").unwrap();
    assert!(!h.args("check").output().unwrap().status.success());

    let mut c = h.args("set-pin").stdin(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    write!(c.stdin.take().unwrap(), "12\n12\n").unwrap();
    assert!(!c.wait().unwrap().success(), "short PIN refused");
    let mut c = h.args("set-pin").stdin(Stdio::piped()).stderr(Stdio::null()).spawn().unwrap();
    write!(c.stdin.take().unwrap(), "1234\n4321\n").unwrap();
    assert!(!c.wait().unwrap().success(), "mismatch refused");
    assert!(!h.dir.join("state/pin").exists());
}
