// The held pairing agent behind the BlueZ backend: one long-lived
// `bluetoothctl` whose stdin stays open, so the agent it registers
// (`NoInputNoOutput`, made the default) lives as long as the child does.
// Without a default agent BlueZ pairs without bonding and then refuses the
// pad's HID link ("Rejected connection from !bonded device").
//
// bluetoothctl registers its own agent at startup, and `agent NoInputNoOutput`
// then answers "Agent is already registered" and keeps that one, so the
// session first drops it (`agent off`) and registers ours. Each command waits
// for the previous one's confirmation line, so no command races the startup.
// A reader thread drives that, answers `ensure` once, then drains stdout
// until the child dies. Every child is killed and waited on (no zombies).

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How long the agent gets to confirm it registered.
pub const AGENT_SETTLE: Duration = Duration::from_secs(3);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stage {
    /// Waiting for bluetoothctl's own startup agent.
    Startup,
    /// Sent `agent off`.
    Dropping,
    /// Sent `agent NoInputNoOutput`.
    Registering,
    /// Sent `default-agent`.
    Defaulting,
    Ready,
    Failed,
}

/// One stdout line moves the session along: the new stage and the command to
/// send, if any. Lines carry ANSI colour and prompt redraws, so this matches
/// substrings.
fn advance(stage: Stage, line: &str) -> (Stage, Option<&'static str>) {
    if line.contains("Failed to register agent") || line.contains("Failed to request default agent") {
        return (Stage::Failed, None);
    }
    match stage {
        Stage::Startup if line.contains("Agent registered") => (Stage::Dropping, Some("agent off")),
        Stage::Dropping if line.contains("Agent unregistered") => {
            (Stage::Registering, Some("agent NoInputNoOutput"))
        }
        Stage::Registering if line.contains("Agent registered") => (Stage::Defaulting, Some("default-agent")),
        Stage::Defaulting if line.contains("Default agent request successful") => (Stage::Ready, None),
        s => (s, None),
    }
}

struct Live {
    child: Child,
    ready: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct AgentSlot {
    live: Mutex<Option<Live>>,
}

fn drive(stdin: ChildStdin, out: impl std::io::Read, ready: Arc<AtomicBool>, tx: Sender<Result<(), String>>) {
    let mut stdin = stdin; // held until the child's stdout closes
    let mut stage = Stage::Startup;
    let mut answered = false;
    let mut last = String::new();
    for line in BufReader::new(out).lines() {
        let Ok(line) = line else { break };
        // A prompt redraw glues "Agent registered" to escape codes, never to
        // other text we match, so substring matching is enough.
        if !line.trim().is_empty() {
            last = line.clone();
        }
        let (next, send) = advance(stage, &line);
        stage = next;
        if let Some(cmd) = send {
            if writeln!(stdin, "{cmd}").and_then(|()| stdin.flush()).is_err() {
                break;
            }
        }
        if !answered && matches!(stage, Stage::Ready | Stage::Failed) {
            answered = true;
            let ok = stage == Stage::Ready;
            ready.store(ok, Ordering::SeqCst);
            let _ = tx.send(if ok { Ok(()) } else { Err(strip_ansi(&last)) });
        }
    }
    ready.store(false, Ordering::SeqCst);
    if !answered {
        let _ = tx.send(Err("bluetoothctl exited".into()));
    }
}

fn strip_ansi(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '\u{1b}' {
            for n in it.by_ref() {
                if n.is_ascii_alphabetic() {
                    break;
                }
            }
        } else if !c.is_control() {
            out.push(c);
        }
    }
    out.trim().to_string()
}

fn kill(l: &mut Live) {
    let _ = l.child.kill();
    let _ = l.child.wait();
}

impl AgentSlot {
    /// Makes sure the agent session is up: a no-op when it already is,
    /// otherwise spawns `cmd` and waits up to `settle` for its confirmations.
    pub fn ensure(&self, mut cmd: Command, settle: Duration) -> Result<(), String> {
        let mut g = self.live.lock().unwrap();
        if let Some(l) = g.as_mut() {
            if l.ready.load(Ordering::SeqCst) && matches!(l.child.try_wait(), Ok(None)) {
                return Ok(());
            }
            kill(l);
            *g = None;
        }
        cmd.stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = cmd.spawn().map_err(|e| format!("{:?}: {e}", cmd.get_program()))?;
        let (Some(stdin), Some(out)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("no pipes".into());
        };
        let ready = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let r2 = ready.clone();
        std::thread::spawn(move || drive(stdin, out, r2, tx));
        let mut live = Live { child, ready };
        let r = rx.recv_timeout(settle).unwrap_or_else(|_| Err("timed out".into()));
        match r {
            Ok(()) => {
                *g = Some(live);
                Ok(())
            }
            Err(e) => {
                kill(&mut live);
                Err(e)
            }
        }
    }

    pub fn stop(&self) {
        if let Some(mut l) = self.live.lock().unwrap().take() {
            kill(&mut l);
        }
    }

    #[cfg(test)]
    fn has_child(&self) -> bool {
        self.live.lock().unwrap().is_some()
    }
}

impl Drop for AgentSlot {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_walks_the_four_confirmations() {
        let (s, c) = advance(Stage::Startup, "\x1b[0;94m[x]> \x1b[0mAgent registered");
        assert_eq!((s, c), (Stage::Dropping, Some("agent off")));
        let (s, c) = advance(s, "Agent unregistered");
        assert_eq!((s, c), (Stage::Registering, Some("agent NoInputNoOutput")));
        let (s, c) = advance(s, "Agent registered");
        assert_eq!((s, c), (Stage::Defaulting, Some("default-agent")));
        assert_eq!(advance(s, "Default agent request successful"), (Stage::Ready, None));
    }

    #[test]
    fn unrelated_and_lookalike_lines_change_nothing() {
        for l in ["", "Agent is already registered", "[CHG] Controller X Pairable: yes", "hci0 new_settings: powered"] {
            assert_eq!(advance(Stage::Startup, l), (Stage::Startup, None), "{l:?}");
        }
        // "unregistered" is not "registered".
        assert_eq!(advance(Stage::Startup, "Agent unregistered"), (Stage::Startup, None));
        assert_eq!(advance(Stage::Registering, "Agent unregistered"), (Stage::Registering, None));
    }

    #[test]
    fn failures_fail_from_any_stage() {
        assert_eq!(advance(Stage::Registering, "Failed to register agent object").0, Stage::Failed);
        assert_eq!(advance(Stage::Defaulting, "Failed to request default agent: x").0, Stage::Failed);
    }

    #[test]
    fn ansi_is_stripped_from_error_text() {
        assert_eq!(strip_ansi("\x1b[0;94m[bt]> \x1b[0mFailed to register agent object\r"), "[bt]> Failed to register agent object");
    }

    #[cfg(unix)]
    fn sh(script: &str) -> Command {
        let mut c = Command::new("sh");
        c.args(["-c", script]);
        c
    }
    #[cfg(unix)]
    const FAKE: &str = "echo 'Agent registered'; read a; echo 'Agent unregistered'; read b; echo 'Agent registered'; \
                        read c; echo 'Default agent request successful'; sleep 30";

    #[cfg(unix)]
    #[test]
    fn ensure_registers_keeps_the_child_and_stop_reaps_it() {
        let slot = AgentSlot::default();
        slot.ensure(sh(FAKE), Duration::from_secs(3)).unwrap();
        assert!(slot.has_child());
        slot.ensure(sh("exit 1"), Duration::from_secs(3)).unwrap(); // already up: not respawned
        slot.stop();
        assert!(!slot.has_child());
    }

    #[cfg(unix)]
    #[test]
    fn a_silent_child_times_out_and_is_reaped() {
        let slot = AgentSlot::default();
        let e = slot.ensure(sh("sleep 30"), Duration::from_millis(300)).unwrap_err();
        assert_eq!(e, "timed out");
        assert!(!slot.has_child());
    }

    #[cfg(unix)]
    #[test]
    fn a_child_that_exits_early_is_an_error() {
        let slot = AgentSlot::default();
        assert_eq!(slot.ensure(sh("exit 1"), Duration::from_secs(3)).unwrap_err(), "bluetoothctl exited");
        assert!(!slot.has_child());
    }

    #[cfg(unix)]
    #[test]
    fn a_failed_registration_reports_its_line() {
        let slot = AgentSlot::default();
        let e = slot.ensure(sh("echo 'Failed to register agent object'; sleep 30"), Duration::from_secs(3)).unwrap_err();
        assert_eq!(e, "Failed to register agent object");
        assert!(!slot.has_child());
    }

    #[cfg(unix)]
    #[test]
    fn a_dead_session_is_respawned() {
        let slot = AgentSlot::default();
        slot.ensure(sh(&FAKE.replace("sleep 30", "sleep 0.3")), Duration::from_secs(3)).unwrap();
        std::thread::sleep(Duration::from_millis(600));
        slot.ensure(sh(FAKE), Duration::from_secs(3)).unwrap();
        assert!(slot.has_child());
        slot.stop();
    }
}
