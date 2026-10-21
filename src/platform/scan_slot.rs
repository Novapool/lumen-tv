// The one held discovery child (`bluetoothctl --timeout N scan on`) behind
// the BlueZ backend. BlueZ ends a discovery session when the D-Bus client
// that started it disconnects, so the child has to stay alive for the scan.
//
// All state sits under one Mutex: `wanted` is false after any stop, so a
// start that was still settling when `stop()` ran sees it and kills nothing
// twice and keeps nothing. Every child is waited on (no zombies).

use std::io::Read;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// A child that exits this much before its own timeout counts as a failure.
const EARLY_SLACK: Duration = Duration::from_secs(2);

#[derive(Default)]
struct State {
    wanted: bool,
    child: Option<Child>,
    started: Option<Instant>,
    secs: u32,
    /// Bumped on every start, so a settling start knows if it was replaced.
    gen: u64,
}

#[derive(Default)]
pub struct ScanSlot {
    state: Mutex<State>,
}

/// Why an exited child counts as an error: its stderr, else its status.
fn exit_text(status: ExitStatus, child: &mut Child) -> String {
    let mut err = String::new();
    if let Some(mut s) = child.stderr.take() {
        let _ = s.read_to_string(&mut err);
    }
    let err = err.trim();
    if err.is_empty() {
        format!("scan process exited ({status})")
    } else {
        err.to_string()
    }
}

impl State {
    fn kill(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl ScanSlot {
    /// Spawns `cmd` (stderr is piped here; stdin/stdout are nulled), waits
    /// `settle`, and returns Err with its stderr if it already exited. A stop
    /// during the wait wins: the scan is not kept and Ok is returned.
    pub fn start(&self, mut cmd: Command, secs: u32, settle: Duration) -> Result<(), String> {
        cmd.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::piped());
        let gen = {
            let mut s = self.state.lock().unwrap();
            s.kill();
            s.wanted = true;
            let child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    s.wanted = false;
                    return Err(format!("{:?}: {e}", cmd.get_program()));
                }
            };
            s.child = Some(child);
            s.started = Some(Instant::now());
            s.secs = secs;
            s.gen += 1;
            s.gen
        };
        std::thread::sleep(settle);
        let mut s = self.state.lock().unwrap();
        if !s.wanted || s.gen != gen {
            return Ok(());
        }
        if let Some(c) = s.child.as_mut() {
            if let Ok(Some(status)) = c.try_wait() {
                let text = exit_text(status, c);
                s.child = None;
                s.wanted = false;
                return Err(text);
            }
        }
        Ok(())
    }

    /// Stops any scan and any start still settling.
    pub fn stop(&self) {
        let mut s = self.state.lock().unwrap();
        s.wanted = false;
        s.kill();
    }

    /// If the child died before its time (non-zero exit, or well before its
    /// timeout), reaps it and returns why, once. A normal end returns None.
    pub fn take_error(&self) -> Option<String> {
        let mut s = self.state.lock().unwrap();
        let early = s.started.is_some_and(|t| t.elapsed() + EARLY_SLACK < Duration::from_secs(u64::from(s.secs)));
        let c = s.child.as_mut()?;
        let status = c.try_wait().ok()??;
        let text = exit_text(status, c);
        s.child = None;
        s.wanted = false;
        (!status.success() || early).then_some(text)
    }

    #[cfg(test)]
    fn has_child(&self) -> bool {
        self.state.lock().unwrap().child.is_some()
    }
}

impl Drop for ScanSlot {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    }
    const SETTLE: Duration = Duration::from_millis(300);

    #[test]
    fn early_exit_returns_stderr() {
        let slot = ScanSlot::default();
        let e = slot.start(sh("echo 'No default controller available' >&2; exit 1"), 60, SETTLE).unwrap_err();
        assert_eq!(e, "No default controller available");
        assert!(!slot.has_child(), "reaped");
        assert!(slot.take_error().is_none());
    }

    #[test]
    fn early_exit_without_stderr_names_the_status() {
        let slot = ScanSlot::default();
        let e = slot.start(sh("exit 3"), 60, SETTLE).unwrap_err();
        assert!(e.starts_with("scan process exited"), "{e}");
    }

    #[test]
    fn running_child_is_kept_and_stop_reaps_it() {
        let slot = ScanSlot::default();
        slot.start(sh("sleep 30"), 60, SETTLE).unwrap();
        assert!(slot.has_child());
        assert!(slot.take_error().is_none(), "alive: no error");
        slot.stop();
        assert!(!slot.has_child());
    }

    #[test]
    fn a_child_that_dies_during_the_scan_is_reported_once() {
        let slot = ScanSlot::default();
        slot.start(sh("sleep 0.6; echo boom >&2; exit 1"), 60, SETTLE).unwrap();
        std::thread::sleep(Duration::from_millis(900));
        assert_eq!(slot.take_error().as_deref(), Some("boom"));
        assert!(slot.take_error().is_none());
        assert!(!slot.has_child());
    }

    #[test]
    fn a_normal_timeout_end_is_not_an_error() {
        let slot = ScanSlot::default();
        // secs = 1: exiting cleanly after ~1 s is within EARLY_SLACK.
        slot.start(sh("sleep 0.6"), 1, Duration::from_millis(100)).unwrap();
        std::thread::sleep(Duration::from_millis(800));
        assert!(slot.take_error().is_none());
        assert!(!slot.has_child(), "reaped even though not an error");
    }

    #[test]
    fn stop_wins_over_an_in_flight_start() {
        let slot = Arc::new(ScanSlot::default());
        let s2 = slot.clone();
        let t = std::thread::spawn(move || s2.start(sh("sleep 30"), 60, Duration::from_millis(500)));
        std::thread::sleep(Duration::from_millis(150));
        slot.stop();
        assert_eq!(t.join().unwrap(), Ok(()));
        assert!(!slot.has_child(), "nothing outlives the stop");
    }

    #[test]
    fn a_second_start_replaces_the_first() {
        let slot = Arc::new(ScanSlot::default());
        let s2 = slot.clone();
        let t = std::thread::spawn(move || s2.start(sh("sleep 30"), 60, Duration::from_millis(500)));
        std::thread::sleep(Duration::from_millis(100));
        slot.start(sh("sleep 30"), 60, Duration::from_millis(10)).unwrap();
        assert_eq!(t.join().unwrap(), Ok(()));
        assert!(slot.has_child(), "the newer scan survives");
        slot.stop();
    }

    #[test]
    fn spawn_failure_is_an_error() {
        let slot = ScanSlot::default();
        assert!(slot.start(Command::new("/nonexistent/scan"), 60, SETTLE).is_err());
        assert!(!slot.has_child());
    }
}
