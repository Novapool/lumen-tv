//! Keeps the HDMI output at the desired mode.
//!
//! The TV resets labwc's output to its own preferred mode (4K) whenever it
//! power-cycles, so a background thread polls `wlr-randr` every ~2 s and
//! re-applies the mode when the output is enabled but at a different mode.
//! The decision is the pure [`decide`]; the thread only runs commands.
//!
//! API for the UI: [`start`] once, [`request_mode`] to change the target (applied
//! promptly), [`status`] for a snapshot. Off Linux everything is a no-op and the
//! status says "n/a".

use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use crate::diag::{parse_wlr_randr, Output};

const POLL: Duration = Duration::from_secs(2);
/// Failed applies in a row before the watcher stops trying.
const MAX_ATTEMPTS: u32 = 3;

/// What the UI can show about the watcher.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DisplayStatus {
    /// False off Linux (no wlr-randr): show "n/a".
    pub available: bool,
    /// `WIDTHxHEIGHT@RATE` the watcher keeps the output at.
    pub desired: String,
    /// Current mode as wlr-randr reports it ("1920x1080 @ 60.00 Hz"); None if the output is missing or disabled.
    pub current: Option<String>,
    /// Last apply result ("applied 1920x1080@60", or the error); None before the first apply.
    pub last_result: Option<String>,
    /// Whether the last apply (or output read) succeeded; None before the first.
    pub last_ok: Option<bool>,
    /// The watcher hit the attempt cap with the output still at the wrong mode.
    pub gave_up: bool,
    /// Number of times a re-apply ran since start.
    pub reapply_count: u32,
}

/// What the poll loop does after an observation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Action {
    Nothing,
    Apply,
}

/// What the loop remembers between polls.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WatchState {
    present: bool,
    desired: String,
    /// Applies tried since the output last matched, appeared or the target changed.
    attempts: u32,
}

/// `(width, height, rate)` of "1920x1080@60" or "1920x1080 @ 59.94 Hz".
fn parse_res_rate(m: &str) -> Option<((u32, u32), Option<f64>)> {
    let m = m.replace(' ', "");
    let (res, rate) = match m.split_once('@') {
        Some((r, rate)) => (r, Some(rate.trim_end_matches("Hz").parse::<f64>().ok()?)),
        None => (m.as_str(), None),
    };
    let (w, h) = res.split_once('x')?;
    Some(((w.parse().ok()?, h.parse().ok()?), rate))
}

/// Does the current mode (wlr-randr text) satisfy `desired`? The rate compares
/// rounded, since 60 Hz modes report 59.94; a desired mode without a rate only
/// checks the size.
pub fn mode_matches(current: &str, desired: &str) -> bool {
    let (Some((cres, crate_)), Some((dres, drate))) = (parse_res_rate(current), parse_res_rate(desired)) else {
        return false;
    };
    cres == dres
        && match (crate_, drate) {
            (Some(c), Some(d)) => c.round() == d.round(),
            _ => true,
        }
}

/// Decides from one poll. `outputs` is the parsed `wlr-randr`; updates `state`.
pub fn decide(state: &mut WatchState, outputs: &[Output], output: &str, desired: &str) -> Action {
    if state.desired != desired {
        state.desired = desired.to_string();
        state.attempts = 0;
    }
    let cur = outputs.iter().find(|o| o.name == output && o.enabled).and_then(|o| o.mode.as_deref());
    let Some(cur) = cur else {
        // Gone (TV off): start fresh when it comes back.
        state.present = false;
        state.attempts = 0;
        return Action::Nothing;
    };
    if !state.present {
        state.present = true;
        state.attempts = 0;
    }
    if mode_matches(cur, desired) {
        state.attempts = 0;
        return Action::Nothing;
    }
    if state.attempts >= MAX_ATTEMPTS {
        return Action::Nothing;
    }
    state.attempts += 1;
    Action::Apply
}

struct Handle {
    tx: Sender<String>,
    status: std::sync::Arc<Mutex<DisplayStatus>>,
}

static WATCHER: OnceLock<Handle> = OnceLock::new();

/// Starts the watcher once (later calls are ignored) at `desired_mode`.
pub fn start(desired_mode: String) {
    let status = std::sync::Arc::new(Mutex::new(DisplayStatus {
        available: cfg!(target_os = "linux"),
        desired: desired_mode.clone(),
        ..Default::default()
    }));
    let (tx, rx) = mpsc::channel::<String>();
    if WATCHER.set(Handle { tx, status: status.clone() }).is_err() {
        return;
    }
    if !cfg!(target_os = "linux") {
        return;
    }
    let output = std::env::var("LUMEN_OUTPUT").ok().filter(|o| !o.trim().is_empty()).unwrap_or_else(|| "HDMI-A-1".into());
    eprintln!("[display] watching {output} for {desired_mode}");
    std::thread::spawn(move || {
        let mut desired = desired_mode;
        let mut state = WatchState::default();
        loop {
            let outputs = read_outputs();
            let current = outputs
                .as_ref()
                .ok()
                .and_then(|o| o.iter().find(|x| x.name == output && x.enabled))
                .and_then(|o| o.mode.clone());
            let action = match &outputs {
                Ok(o) => decide(&mut state, o, &output, &desired),
                Err(_) => Action::Nothing,
            };
            let mut result = None;
            let gave_up = action == Action::Nothing && state.attempts >= MAX_ATTEMPTS;
            if action == Action::Apply {
                let r = apply(&output, &desired);
                match &r {
                    Ok(m) => eprintln!("[display] re-applied {m} on {output}"),
                    Err(e) => eprintln!("[display] could not set {desired} on {output}: {e}"),
                }
                result = Some(r.map(|m| format!("applied {m}")));
            }
            if let Ok(mut s) = status.lock() {
                s.current = current;
                match &outputs {
                    Err(e) => {
                        s.last_result = Some(e.clone());
                        s.last_ok = Some(false);
                    }
                    Ok(_) => s.gave_up = gave_up,
                }
                if let Some(r) = result {
                    s.last_ok = Some(r.is_ok());
                    s.last_result = Some(r.unwrap_or_else(|e| e));
                    s.reapply_count += 1;
                }
            }
            match rx.recv_timeout(POLL) {
                Ok(m) => {
                    eprintln!("[display] target is now {m}");
                    desired = m;
                    while let Ok(m) = rx.try_recv() {
                        desired = m; // only the newest matters
                    }
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => return,
            }
        }
    });
}

/// Changes the target mode (`WIDTHxHEIGHT@RATE`); the watcher applies it right
/// away rather than at the next poll. Does not save it: see `settings::save_mode`.
pub fn request_mode(mode: &str) {
    if let Some(h) = WATCHER.get() {
        if let Ok(mut s) = h.status.lock() {
            s.desired = mode.to_string();
            s.gave_up = false;
        }
        let _ = h.tx.send(mode.to_string());
    }
}

/// A snapshot of the watcher (default, `available: false`, before [`start`]).
pub fn status() -> DisplayStatus {
    WATCHER.get().and_then(|h| h.status.lock().ok().map(|s| s.clone())).unwrap_or_default()
}

fn wlr_randr(args: &[&str]) -> std::process::Command {
    let mut cmd = std::process::Command::new("wlr-randr");
    cmd.args(args).stdin(std::process::Stdio::null());
    crate::apps::drop_ambient_caps(&mut cmd);
    cmd
}

fn read_outputs() -> Result<Vec<Output>, String> {
    match wlr_randr(&[]).output() {
        Ok(o) if o.status.success() => Ok(parse_wlr_randr(&String::from_utf8_lossy(&o.stdout))),
        Ok(o) => Err(format!("wlr-randr exited with {}", o.status)),
        Err(e) => Err(format!("wlr-randr: {e}")),
    }
}

/// `--mode M`, falling back to `--mode WIDTHxHEIGHT` (like start-lumen.sh).
/// Returns the mode string that worked.
fn apply(output: &str, mode: &str) -> Result<String, String> {
    let run = |m: &str| -> Result<(), String> {
        let o = wlr_randr(&["--output", output, "--mode", m]).output().map_err(|e| format!("wlr-randr: {e}"))?;
        if o.status.success() {
            Ok(())
        } else {
            let err = String::from_utf8_lossy(&o.stderr);
            Err(format!("wlr-randr {m}: {}", err.trim().lines().next().unwrap_or("failed")))
        }
    };
    match run(mode) {
        Ok(()) => Ok(mode.to_string()),
        Err(first) => {
            let bare = mode.split('@').next().unwrap_or(mode);
            if bare == mode {
                return Err(first);
            }
            run(bare).map(|()| bare.to_string()).map_err(|e| format!("{first}; {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(mode: Option<&str>, enabled: bool) -> Vec<Output> {
        vec![Output { name: "HDMI-A-1".into(), enabled, mode: mode.map(String::from), scale: None }]
    }
    const D: &str = "1920x1080@60";
    const UHD: &str = "3840x2160 @ 60.00 Hz";
    const FHD: &str = "1920x1080 @ 59.94 Hz";

    #[test]
    fn matches_rounds_rate_and_allows_missing_rate() {
        assert!(mode_matches(FHD, D));
        assert!(mode_matches("1920x1080 @ 60.00 Hz", D));
        assert!(!mode_matches(UHD, D));
        assert!(!mode_matches("1920x1080 @ 30.00 Hz", D));
        assert!(mode_matches("1920x1080 @ 30.00 Hz", "1920x1080"));
        assert!(!mode_matches("garbage", D));
    }

    #[test]
    fn already_correct_does_nothing() {
        let mut st = WatchState::default();
        assert_eq!(decide(&mut st, &out(Some(FHD), true), "HDMI-A-1", D), Action::Nothing);
    }

    #[test]
    fn output_missing_or_disabled_does_nothing() {
        let mut st = WatchState::default();
        assert_eq!(decide(&mut st, &[], "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &out(None, false), "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &out(Some(UHD), false), "HDMI-A-1", D), Action::Nothing);
    }

    #[test]
    fn reappearing_at_4k_applies_1080p() {
        let mut st = WatchState::default();
        assert_eq!(decide(&mut st, &out(Some(FHD), true), "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &[], "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &out(Some(UHD), true), "HDMI-A-1", D), Action::Apply);
    }

    #[test]
    fn gives_up_after_three_then_retries_on_reappear_or_new_target() {
        let mut st = WatchState::default();
        let bad = out(Some(UHD), true);
        for _ in 0..3 {
            assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Apply);
        }
        assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Nothing);
        // output disappears and comes back
        assert_eq!(decide(&mut st, &[], "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Apply);
        for _ in 0..2 {
            decide(&mut st, &bad, "HDMI-A-1", D);
        }
        assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Nothing);
        // desired mode changes
        assert_eq!(decide(&mut st, &out(Some(FHD), true), "HDMI-A-1", "3840x2160@60"), Action::Apply);
    }

    #[test]
    fn other_outputs_are_ignored() {
        let mut st = WatchState::default();
        let outs = vec![Output { name: "HDMI-A-2".into(), enabled: true, mode: Some(UHD.into()), scale: None }];
        assert_eq!(decide(&mut st, &outs, "HDMI-A-1", D), Action::Nothing);
    }

    #[test]
    fn status_before_start_is_unavailable() {
        if WATCHER.get().is_none() {
            assert!(!status().available);
        }
    }


    #[test]
    fn flapping_output_resets_the_attempt_cap_each_time_it_matches() {
        let mut st = WatchState::default();
        let (bad, good) = (out(Some(UHD), true), out(Some(FHD), true));
        for _ in 0..5 {
            for _ in 0..2 {
                assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Apply);
            }
            assert_eq!(decide(&mut st, &good, "HDMI-A-1", D), Action::Nothing);
        }
    }

    #[test]
    fn cap_is_exactly_three_applies_per_episode() {
        let mut st = WatchState::default();
        let bad = out(Some(UHD), true);
        let applies = (0..10).filter(|_| decide(&mut st, &bad, "HDMI-A-1", D) == Action::Apply).count();
        assert_eq!(applies, 3);
    }

    #[test]
    fn disabled_output_mid_episode_resets_the_cap() {
        let mut st = WatchState::default();
        let bad = out(Some(UHD), true);
        for _ in 0..3 {
            decide(&mut st, &bad, "HDMI-A-1", D);
        }
        assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &out(None, false), "HDMI-A-1", D), Action::Nothing);
        assert_eq!(decide(&mut st, &bad, "HDMI-A-1", D), Action::Apply);
    }

    #[test]
    fn enabled_output_without_a_mode_is_treated_as_gone() {
        let mut st = WatchState::default();
        assert_eq!(decide(&mut st, &out(None, true), "HDMI-A-1", D), Action::Nothing);
    }

    #[test]
    fn changing_target_back_to_the_current_mode_stops_applying() {
        let mut st = WatchState::default();
        let uhd = out(Some(UHD), true);
        assert_eq!(decide(&mut st, &uhd, "HDMI-A-1", D), Action::Apply);
        assert_eq!(decide(&mut st, &uhd, "HDMI-A-1", "3840x2160@60"), Action::Nothing);
    }

    #[test]
    fn mode_matches_rejects_malformed_and_odd_rates() {
        assert!(!mode_matches(FHD, "1920x1080@fast"));
        assert!(!mode_matches("", D));
        assert!(!mode_matches(FHD, ""));
        assert!(mode_matches("1920x1080 @ 59.94 Hz", "1920x1080@59.94"));
        assert!(!mode_matches("1920x1080 @ 50.00 Hz", D));
    }
}
