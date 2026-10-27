// Client for the root install helper (`lumen-helper`, see AGENT_JOBS.md M5.3).
//
// std `UnixStream` + serde only; it does not depend on the helper crate.
// The wire protocol (one request line, streamed lines, one final line) is the
// contract in AGENT_JOBS.md "Helper protocol". The PIN is sent in the request
// and never stored, logged or put in an error message. Anything the client
// can't parse is an error, never a default.

use std::collections::BTreeMap;
use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::time::Duration;

use serde::Deserialize;
use serde_json::json;

use super::{RootEvent, RootInstaller, RootOp, RootOutcome, RootSet, RootStatus};

pub const DEFAULT_SOCKET: &str = "/run/lumen-helper.sock";
const PROTOCOL: u32 = 1;
/// Connect limit; also the write limit and the status reply limit.
const QUICK: Duration = Duration::from_secs(3);
/// A silent install can legitimately take long (the helper kills apt at 15 min).
const RUN_SILENCE: Duration = Duration::from_secs(20 * 60);

/// One line from the helper: the first three stream, the rest end the exchange.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum Reply {
    Line(String),
    Replace(String),
    Step(String),
    Status(StatusBody),
    Done(bool),
    Failed { code: i32, message: String },
    BadPin { tries_left: Option<u32>, locked_for_s: u32 },
    Locked { locked_for_s: u32 },
    // The wire value is always `true` (`{"no_pin":true}`); only the tag matters.
    NoPin(#[allow(dead_code)] bool),
    NotAllowed(String),
    Busy(#[allow(dead_code)] bool),
    Error(String),
}

#[derive(Debug, Deserialize)]
struct StatusBody {
    pin_set: bool,
    locked_for_s: u32,
    sets: BTreeMap<String, SetBody>,
}

#[derive(Debug, Deserialize)]
struct SetBody {
    install: Vec<String>,
    remove: Vec<String>,
}

fn invalid(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

/// No helper installed (no socket file, or nobody listening on it).
fn is_missing(e: &io::Error) -> bool {
    matches!(e.kind(), io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused)
}

/// Why the helper couldn't be used, for the Store's note.
fn unreachable_note(e: &io::Error) -> String {
    if e.kind() == io::ErrorKind::PermissionDenied {
        "restart Lumen to pick up the lumen-helper group".into()
    } else if e.kind() == io::ErrorKind::InvalidData {
        e.to_string()
    } else {
        format!("can't reach the helper: {e}")
    }
}

fn outcome_of(reply: Reply) -> RootOutcome {
    match reply {
        Reply::Done(true) => RootOutcome::Done,
        Reply::Failed { code, message } => RootOutcome::Failed { code, message },
        Reply::BadPin { tries_left, locked_for_s } => RootOutcome::BadPin { tries_left, locked_for_s },
        Reply::Locked { locked_for_s } => RootOutcome::Locked { locked_for_s },
        Reply::NoPin(_) => RootOutcome::NoPin,
        Reply::NotAllowed(why) => RootOutcome::NotAllowed(why),
        Reply::Busy(_) => RootOutcome::Busy,
        Reply::Error(why) => RootOutcome::Error(why),
        // `done: false`, a status, or a streamed line as the last line: not a result.
        Reply::Done(false) | Reply::Status(_) | Reply::Line(_) | Reply::Replace(_) | Reply::Step(_) => {
            RootOutcome::Error("unexpected final reply from the helper".into())
        }
    }
}

fn status_of(reply: Reply) -> RootStatus {
    match reply {
        Reply::Status(s) => RootStatus {
            available: true,
            pin_set: s.pin_set,
            locked_for_s: s.locked_for_s,
            sets: s.sets.into_iter().map(|(id, v)| (id, RootSet { install: v.install, remove: v.remove })).collect(),
            note: None,
        },
        // The helper answered but won't serve us (e.g. allowlist unsafe).
        Reply::NotAllowed(why) | Reply::Error(why) => RootStatus::unavailable(Some(why)),
        _ => RootStatus::unavailable(Some("unexpected reply from the helper".into())),
    }
}

pub struct SocketInstaller {
    path: PathBuf,
}

impl SocketInstaller {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        SocketInstaller { path: path.into() }
    }

    /// `LUMEN_HELPER_SOCKET`, else `/run/lumen-helper.sock`.
    pub fn from_env() -> Self {
        Self::new(std::env::var("LUMEN_HELPER_SOCKET").unwrap_or_else(|_| DEFAULT_SOCKET.into()))
    }

    /// std has no connect timeout, so connect on a helper thread and give up
    /// after `QUICK` (the thread ends by itself when the connect returns).
    fn connect(&self, read_timeout: Duration) -> io::Result<UnixStream> {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = self.path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(UnixStream::connect(path));
        });
        let s = rx.recv_timeout(QUICK).map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out connecting"))??;
        s.set_read_timeout(Some(read_timeout))?;
        s.set_write_timeout(Some(QUICK))?;
        Ok(s)
    }

    /// Sends `req`, hands every streamed reply to `on_stream`, returns the final one.
    fn exchange(&self, req: &serde_json::Value, read_timeout: Duration, on_stream: &mut dyn FnMut(Reply)) -> io::Result<Reply> {
        let mut stream = self.connect(read_timeout)?;
        let mut text = req.to_string();
        text.push('\n');
        stream.write_all(text.as_bytes())?;
        let mut reader = BufReader::new(stream);
        let mut buf = Vec::new();
        loop {
            buf.clear();
            if reader.read_until(b'\n', &mut buf)? == 0 {
                return Err(invalid("the helper closed the connection before its final reply"));
            }
            let line = String::from_utf8_lossy(&buf);
            if line.trim().is_empty() {
                continue;
            }
            let reply: Reply = serde_json::from_str(line.trim()).map_err(|e| invalid(format!("bad reply from the helper: {e}")))?;
            if matches!(reply, Reply::Line(_) | Reply::Replace(_) | Reply::Step(_)) {
                on_stream(reply);
            } else {
                return Ok(reply);
            }
        }
    }
}

impl RootInstaller for SocketInstaller {
    fn status(&self) -> RootStatus {
        let req = json!({ "v": PROTOCOL, "op": "status" });
        match self.exchange(&req, QUICK, &mut |_| {}) {
            Ok(reply) => status_of(reply),
            Err(e) if is_missing(&e) => RootStatus::unavailable(None),
            Err(e) => RootStatus::unavailable(Some(unreachable_note(&e))),
        }
    }

    fn run(&self, op: RootOp, id: &str, pin: &str, on_event: &dyn Fn(RootEvent)) -> RootOutcome {
        let req = json!({ "v": PROTOCOL, "op": op.as_str(), "id": id, "pin": pin });
        let mut stream = |r: Reply| match r {
            Reply::Line(t) => on_event(RootEvent::Line(t)),
            Reply::Replace(t) => on_event(RootEvent::Replace(t)),
            Reply::Step(t) => on_event(RootEvent::Step(t)),
            _ => {}
        };
        match self.exchange(&req, RUN_SILENCE, &mut stream) {
            Ok(reply) => outcome_of(reply),
            Err(e) if is_missing(&e) => RootOutcome::Unavailable,
            Err(e) => RootOutcome::Error(unreachable_note(&e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;
    use std::os::unix::net::UnixListener;
    use std::sync::{Arc, Mutex};

    /// A fake helper: reads the request line, records it, writes `reply`
    /// verbatim, closes.
    fn serve(tag: &str, reply: &'static str) -> (PathBuf, Arc<Mutex<Vec<String>>>) {
        let path = std::env::temp_dir().join(format!("lh-{tag}-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(mut c) = conn else { break };
                let mut line = String::new();
                BufReader::new(c.try_clone().unwrap()).read_line(&mut line).unwrap();
                log.lock().unwrap().push(line);
                let _ = c.write_all(reply.as_bytes());
            }
        });
        (path, seen)
    }

    fn reply(wire: &str) -> Reply {
        serde_json::from_str(wire).unwrap()
    }

    #[test]
    fn missing_socket_is_unavailable_not_an_error() {
        let c = SocketInstaller::new(std::env::temp_dir().join("lh-nonexistent.sock"));
        let s = c.status();
        assert!(!s.available && s.note.is_none());
        assert_eq!(c.run(RootOp::Install, "x", "1234", &|_| {}), RootOutcome::Unavailable);
    }

    #[test]
    fn stale_socket_file_is_unavailable() {
        let path = std::env::temp_dir().join(format!("lh-stale-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        drop(UnixListener::bind(&path).unwrap()); // leaves the file, nobody listening
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available);
        // macOS std sets CLOEXEC after creating the socket, so a `sh` spawned by a
        // concurrent store test can inherit the listener and keep it alive: the
        // connect then hangs or answers differently. Only Linux makes it exact.
        #[cfg(target_os = "linux")]
        assert!(s.note.is_none());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn permission_denied_says_to_restart_lumen() {
        use std::os::unix::fs::PermissionsExt;
        let (path, _) = serve("perm", "{\"done\":true}\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        if UnixStream::connect(&path).is_ok() {
            // running as root: permissions don't apply
            let _ = std::fs::remove_file(&path);
            return;
        }
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available);
        assert!(s.note.as_deref().is_some_and(|n| n.contains("restart Lumen")), "{:?}", s.note);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn status_parses_sets_pin_and_lock() {
        let (path, seen) = serve(
            "status",
            "{\"status\":{\"pin_set\":true,\"locked_for_s\":42,\"sets\":{\"root-test\":{\"install\":[\"sl\"],\"remove\":[\"sl\"]},\"airplay\":{\"install\":[\"uxplay\",\"gstreamer1.0-libav\"],\"remove\":[]}}}}\n",
        );
        let s = SocketInstaller::new(&path).status();
        assert!(s.available && s.pin_set);
        assert_eq!(s.locked_for_s, 42);
        assert_eq!(s.sets["root-test"], RootSet { install: vec!["sl".into()], remove: vec!["sl".into()] });
        assert_eq!(s.sets["airplay"].install.len(), 2);
        assert!(s.sets["airplay"].remove.is_empty());
        let req: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
        assert_eq!(req, json!({"v": 1, "op": "status"}));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn status_error_replies_become_a_note() {
        let (path, _) = serve("st-na", "{\"not_allowed\":\"allowlist is writable\"}\n");
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available && s.note.as_deref() == Some("allowlist is writable"));
        let _ = std::fs::remove_file(&path);
        let (path, _) = serve("st-err", "{\"error\":\"malformed request\"}\n");
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available && s.note.as_deref() == Some("malformed request"));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn incomplete_status_is_not_trusted() {
        // No `sets`: fail closed, don't default to "empty allowlist, pin set".
        let (path, _) = serve("st-bad", "{\"status\":{\"pin_set\":true}}\n");
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available && s.note.is_some());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn install_streams_events_and_sends_the_request() {
        let (path, seen) = serve(
            "install",
            "{\"step\":\"apt-get install -y -- sl\"}\n{\"line\":\"Reading package lists\"}\n{\"line\":\"10%\"}\n{\"replace\":\"90%\"}\n{\"done\":true}\n",
        );
        let events = Mutex::new(Vec::new());
        let out = SocketInstaller::new(&path).run(RootOp::Install, "root-test", "4821", &|e| events.lock().unwrap().push(e));
        assert_eq!(out, RootOutcome::Done);
        assert_eq!(
            events.into_inner().unwrap(),
            vec![
                RootEvent::Step("apt-get install -y -- sl".into()),
                RootEvent::Line("Reading package lists".into()),
                RootEvent::Line("10%".into()),
                RootEvent::Replace("90%".into()),
            ]
        );
        let req: Value = serde_json::from_str(&seen.lock().unwrap()[0]).unwrap();
        assert_eq!(req, json!({"v": 1, "op": "install", "id": "root-test", "pin": "4821"}));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn remove_uses_the_remove_op() {
        let (path, seen) = serve("remove", "{\"done\":true}\n");
        assert_eq!(SocketInstaller::new(&path).run(RootOp::Remove, "kodi", "1234", &|_| {}), RootOutcome::Done);
        assert!(seen.lock().unwrap()[0].contains("\"op\":\"remove\""));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn every_final_line_maps_to_an_outcome() {
        let cases = [
            ("{\"done\":true}", RootOutcome::Done),
            ("{\"failed\":{\"code\":100,\"message\":\"E: Unable to locate package\"}}", RootOutcome::Failed { code: 100, message: "E: Unable to locate package".into() }),
            ("{\"bad_pin\":{\"tries_left\":3,\"locked_for_s\":0}}", RootOutcome::BadPin { tries_left: Some(3), locked_for_s: 0 }),
            ("{\"bad_pin\":{\"tries_left\":null,\"locked_for_s\":60}}", RootOutcome::BadPin { tries_left: None, locked_for_s: 60 }),
            ("{\"locked\":{\"locked_for_s\":55}}", RootOutcome::Locked { locked_for_s: 55 }),
            ("{\"no_pin\":true}", RootOutcome::NoPin),
            ("{\"not_allowed\":\"unknown id\"}", RootOutcome::NotAllowed("unknown id".into())),
            ("{\"busy\":true}", RootOutcome::Busy),
            ("{\"error\":\"malformed request\"}", RootOutcome::Error("malformed request".into())),
            ("{\"done\":false}", RootOutcome::Error("unexpected final reply from the helper".into())),
        ];
        for (wire, want) in cases {
            assert_eq!(outcome_of(reply(wire)), want, "{wire}");
        }
    }

    #[test]
    fn malformed_lines_do_not_parse() {
        for wire in ["{\"mystery\":1}", "{\"failed\":{\"code\":1}}", "{\"bad_pin\":{}}", "{\"done\":true,\"busy\":true}", "[]", "not json"] {
            assert!(serde_json::from_str::<Reply>(wire).is_err(), "{wire}");
        }
    }

    #[test]
    fn early_close_and_garbage_are_errors_not_done() {
        let (path, _) = serve("early", "{\"line\":\"half\"}\n");
        let out = SocketInstaller::new(&path).run(RootOp::Install, "x", "1234", &|_| {});
        assert!(matches!(out, RootOutcome::Error(m) if m.contains("closed")));
        let _ = std::fs::remove_file(&path);

        let (path, _) = serve("garbage", "not json\n");
        let out = SocketInstaller::new(&path).run(RootOp::Install, "x", "1234", &|_| {});
        assert!(matches!(out, RootOutcome::Error(_)));
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available && s.note.is_some());
        let _ = std::fs::remove_file(&path);

        let (path, _) = serve("mystery", "{\"mystery\":1}\n");
        let out = SocketInstaller::new(&path).run(RootOp::Install, "x", "1234", &|_| {});
        assert!(matches!(out, RootOutcome::Error(_)));
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_silent_helper_times_out_the_status_call() {
        let path = std::env::temp_dir().join(format!("lh-silent-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let listener = UnixListener::bind(&path).unwrap();
        let hold = std::thread::spawn(move || {
            let c = listener.accept();
            std::thread::sleep(Duration::from_secs(5));
            drop(c);
        });
        let t = std::time::Instant::now();
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available && t.elapsed() < Duration::from_secs(4), "{:?}", t.elapsed());
        let _ = hold.join();
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn blank_lines_in_the_stream_are_skipped() {
        let (path, _) = serve("blank", "\n{\"line\":\"a\"}\n   \n\n{\"done\":true}\n");
        let events = Mutex::new(Vec::new());
        let out = SocketInstaller::new(&path).run(RootOp::Install, "x", "1234", &|e| events.lock().unwrap().push(e));
        assert_eq!(out, RootOutcome::Done);
        assert_eq!(events.into_inner().unwrap(), vec![RootEvent::Line("a".into())]);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn nothing_after_the_final_line_is_acted_on() {
        let (path, _) = serve("after-final", "{\"bad_pin\":{\"tries_left\":2,\"locked_for_s\":0}}\n{\"done\":true}\n");
        let out = SocketInstaller::new(&path).run(RootOp::Install, "x", "1234", &|_| {});
        assert_eq!(out, RootOutcome::BadPin { tries_left: Some(2), locked_for_s: 0 });
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_reply_of_the_wrong_kind_is_an_error_for_both_calls() {
        // run() given a status: not a result.
        let (path, _) = serve("run-status", "{\"status\":{\"pin_set\":true,\"locked_for_s\":0,\"sets\":{}}}\n");
        assert!(matches!(SocketInstaller::new(&path).run(RootOp::Install, "x", "1234", &|_| {}), RootOutcome::Error(_)));
        let _ = std::fs::remove_file(&path);
        // status() given done / busy / bad_pin: not trusted, never "available".
        for wire in ["{\"done\":true}\n", "{\"busy\":true}\n", "{\"bad_pin\":{\"tries_left\":1,\"locked_for_s\":0}}\n", "{\"no_pin\":true}\n"] {
            let (path, _) = serve("status-wrong", wire);
            let s = SocketInstaller::new(&path).status();
            assert!(!s.available && !s.pin_set && s.sets.is_empty() && s.note.is_some(), "{wire}");
            let _ = std::fs::remove_file(&path);
        }
    }

    #[test]
    fn out_of_range_numbers_do_not_parse() {
        for wire in [
            "{\"bad_pin\":{\"tries_left\":-1,\"locked_for_s\":0}}",
            "{\"bad_pin\":{\"tries_left\":1,\"locked_for_s\":-5}}",
            "{\"locked\":{\"locked_for_s\":-1}}",
            "{\"locked\":{\"locked_for_s\":99999999999}}",
            "{\"status\":{\"pin_set\":\"yes\",\"locked_for_s\":0,\"sets\":{}}}",
            "{\"status\":{\"pin_set\":true,\"locked_for_s\":0,\"sets\":{\"a\":{\"install\":\"sl\",\"remove\":[]}}}}",
            "{\"status\":{\"pin_set\":true,\"locked_for_s\":0,\"sets\":{\"a\":{\"install\":[\"sl\"]}}}}",
        ] {
            assert!(serde_json::from_str::<Reply>(wire).is_err(), "{wire}");
        }
    }

    #[test]
    fn a_status_with_a_missing_remove_list_is_not_trusted() {
        let (path, _) = serve("st-noremove", "{\"status\":{\"pin_set\":true,\"locked_for_s\":0,\"sets\":{\"a\":{\"install\":[\"sl\"]}}}}\n");
        let s = SocketInstaller::new(&path).status();
        assert!(!s.available && s.sets.is_empty());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn the_pin_is_in_the_request_only_and_never_in_what_comes_back() {
        // Failure paths: refused, garbage, early close, nobody listening.
        let pin = "73519";
        let wires: [&'static str; 4] = ["{\"error\":\"malformed request\"}\n", "garbage\n", "{\"line\":\"x\"}\n", "{\"failed\":{\"code\":1,\"message\":\"E: x\"}}\n"];
        for (i, wire) in wires.iter().enumerate() {
            let (path, seen) = serve(&format!("pinleak{i}"), wire);
            let events = Mutex::new(Vec::new());
            let out = SocketInstaller::new(&path).run(RootOp::Remove, "kodi", pin, &|e| events.lock().unwrap().push(e));
            assert!(seen.lock().unwrap()[0].contains(pin), "the request carries it");
            assert!(!format!("{out:?}{:?}", events.into_inner().unwrap()).contains(pin), "{wire}");
            let _ = std::fs::remove_file(&path);
        }
        let gone = SocketInstaller::new(std::env::temp_dir().join("lh-pinleak-none.sock")).run(RootOp::Install, "x", pin, &|_| {});
        assert!(!format!("{gone:?}").contains(pin));
    }

    #[test]
    fn the_request_is_one_json_line_with_exactly_the_contract_fields() {
        let (path, seen) = serve("one-line", "{\"done\":true}\n");
        SocketInstaller::new(&path).run(RootOp::Remove, "root-test", "1234", &|_| {});
        let raw = seen.lock().unwrap()[0].clone();
        assert!(raw.ends_with('\n') && raw.matches('\n').count() == 1);
        let v: Value = serde_json::from_str(&raw).unwrap();
        let mut keys: Vec<_> = v.as_object().unwrap().keys().cloned().collect();
        keys.sort();
        assert_eq!(keys, ["id", "op", "pin", "v"]);
        let _ = std::fs::remove_file(&path);
    }
}
