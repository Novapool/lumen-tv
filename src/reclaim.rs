// When to claim the TV remote again after an app exits. Kodi (libcec)
// unregisters the CEC adapter on exit and tells the TV to leave the Pi's
// input, so Lumen claims the remote once more. Pure, so it is tested here;
// `session.rs` feeds it and runs the claim.

/// Minimum gap between two claims.
pub const DEBOUNCE_MS: u64 = 3000;

#[derive(Default)]
pub struct Reclaim {
    /// An app exited and the remote has not been claimed since.
    due: bool,
    /// A claim (startup or re-claim) is running right now.
    in_flight: bool,
    last: Option<u64>,
}

impl Reclaim {
    /// An app process exited.
    pub fn mark_due(&mut self) {
        self.due = true;
    }

    /// A claim starts outside `poll` (the one at startup).
    pub fn begin(&mut self, now_ms: u64) {
        self.in_flight = true;
        self.last = Some(now_ms);
    }

    /// The running claim finished.
    pub fn finished(&mut self) {
        self.in_flight = false;
    }

    /// Returns true if a re-claim should start now. `ready` = Lumen is in
    /// front and no launch is pending. While not ready, claiming, or inside the
    /// debounce window, a due claim stays due and waits for the next call.
    pub fn poll(&mut self, ready: bool, now_ms: u64) -> bool {
        if !self.due || !ready || self.in_flight {
            return false;
        }
        if self
            .last
            .is_some_and(|l| now_ms.saturating_sub(l) < DEBOUNCE_MS)
        {
            return false;
        }
        self.due = false;
        self.begin(now_ms);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fires_once_per_exit_when_ready() {
        let mut r = Reclaim::default();
        assert!(!r.poll(true, 100)); // nothing due
        r.mark_due();
        assert!(!r.poll(false, 1000)); // an app is in front or launching: wait
        assert!(r.poll(true, 9500)); // Lumen is back: fire
        r.finished();
        assert!(!r.poll(true, 9600)); // already claimed
    }

    #[test]
    fn a_debounced_exit_is_deferred_not_dropped() {
        let mut r = Reclaim::default();
        r.mark_due();
        assert!(r.poll(true, 5000));
        r.finished();
        r.mark_due();
        assert!(!r.poll(true, 5500)); // inside the window
        assert!(r.poll(true, 5000 + DEBOUNCE_MS)); // still due afterwards
    }

    #[test]
    fn never_overlaps_a_claim_in_flight() {
        let mut r = Reclaim::default();
        r.begin(0); // startup claim
        r.mark_due();
        assert!(!r.poll(true, 10_000)); // still running
        r.finished();
        assert!(r.poll(true, 10_001));
        r.mark_due();
        assert!(!r.poll(true, 20_000)); // the re-claim has not finished
    }
}
