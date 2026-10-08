// Turns presses and releases from all devices into the actions the UI sees:
// directions repeat while held (speeding up), Back fires on release, and
// holding Back fires Home instead. Pure logic; the caller supplies the time
// and wakes up at deadline() to call tick().

use std::time::{Duration, Instant};

use super::Action;

const REPEAT_DELAY: Duration = Duration::from_millis(400);
const REPEAT_START: Duration = Duration::from_millis(160);
const REPEAT_MIN: Duration = Duration::from_millis(60);
/// Each repeat comes this much sooner than the one before, down to REPEAT_MIN.
const REPEAT_ACCEL: f64 = 0.85;
const LONG_PRESS: Duration = Duration::from_millis(800);

/// `S` identifies the device a press came from, so unplugging a device or
/// releasing on another one can't leave a direction stuck.
pub struct Dispatcher<S> {
    held: Option<Held<S>>,
    back: Option<(S, Instant)>,
}

struct Held<S> {
    source: S,
    action: Action,
    next: Instant,
    repeats: i32,
}

impl<S: Clone + PartialEq> Default for Dispatcher<S> {
    fn default() -> Self {
        Dispatcher { held: None, back: None }
    }
}

impl<S: Clone + PartialEq> Dispatcher<S> {
    pub fn press(&mut self, source: &S, action: Action, now: Instant) -> Option<Action> {
        if action.is_direction() {
            // The latest direction wins; it alone repeats.
            self.held = Some(Held { source: source.clone(), action, next: now + REPEAT_DELAY, repeats: 0 });
            Some(action)
        } else if action == Action::Back {
            self.back = Some((source.clone(), now));
            None
        } else {
            Some(action)
        }
    }

    pub fn release(&mut self, source: &S, action: Action, now: Instant) -> Option<Action> {
        if action.is_direction() {
            if self.held.as_ref().is_some_and(|h| h.source == *source && h.action == action) {
                self.held = None;
            }
            None
        } else if action == Action::Back {
            // None here means the long press already fired Home.
            match self.back.take() {
                Some((s, since)) if s == *source && now - since < LONG_PRESS => Some(Action::Back),
                other => {
                    self.back = other.filter(|(s, _)| s != source);
                    None
                }
            }
        } else {
            None
        }
    }

    /// Forgets everything a device was holding (it was unplugged).
    pub fn release_device(&mut self, source: &S) {
        if self.held.as_ref().is_some_and(|h| h.source == *source) {
            self.held = None;
        }
        if self.back.as_ref().is_some_and(|(s, _)| s == source) {
            self.back = None;
        }
    }

    /// When tick() next has something to do.
    pub fn deadline(&self) -> Option<Instant> {
        let repeat = self.held.as_ref().map(|h| h.next);
        let long_press = self.back.as_ref().map(|(_, since)| *since + LONG_PRESS);
        repeat.into_iter().chain(long_press).min()
    }

    pub fn tick(&mut self, now: Instant) -> Vec<(S, Action)> {
        let mut out = Vec::new();
        if let Some(h) = self.held.as_mut().filter(|h| h.next <= now) {
            out.push((h.source.clone(), h.action));
            h.repeats += 1;
            let interval = REPEAT_START.mul_f64(REPEAT_ACCEL.powi(h.repeats - 1));
            h.next = now + interval.max(REPEAT_MIN);
        }
        if let Some((source, _)) = self.back.take_if(|(_, since)| *since + LONG_PRESS <= now) {
            out.push((source, Action::Home));
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Runs tick() at every deadline up to `until`, collecting the actions with their times.
    fn run_until(d: &mut Dispatcher<u8>, t0: Instant, until: Duration) -> Vec<(Duration, Action)> {
        let mut out = Vec::new();
        while let Some(at) = d.deadline().filter(|&at| at <= t0 + until) {
            out.extend(d.tick(at).into_iter().map(|(_, a)| (at - t0, a)));
        }
        out
    }

    #[test]
    fn tap_fires_once() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        assert_eq!(d.press(&1, Action::Right, t0), Some(Action::Right));
        assert_eq!(d.release(&1, Action::Right, t0 + ms(100)), None);
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn hold_repeats_faster_until_released() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        d.press(&1, Action::Right, t0);
        let fired = run_until(&mut d, t0, ms(1500));
        assert!(fired.iter().all(|&(_, a)| a == Action::Right));
        assert_eq!(fired[0].0, REPEAT_DELAY);
        let gaps: Vec<_> = fired.windows(2).map(|w| w[1].0 - w[0].0).collect();
        assert_eq!(gaps[0], REPEAT_START);
        assert!(gaps.windows(2).all(|g| g[1] <= g[0]), "intervals never grow: {gaps:?}");
        assert_eq!(*gaps.last().unwrap(), REPEAT_MIN);

        d.release(&1, Action::Right, t0 + ms(1500));
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn latest_direction_wins() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        d.press(&1, Action::Right, t0);
        d.press(&1, Action::Down, t0 + ms(100));
        d.release(&1, Action::Right, t0 + ms(200)); // not the repeating one: no effect
        let fired = run_until(&mut d, t0, ms(600));
        assert_eq!(fired, vec![(ms(500), Action::Down)]);
    }

    #[test]
    fn short_back_fires_on_release() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        assert_eq!(d.press(&1, Action::Back, t0), None);
        assert_eq!(d.release(&1, Action::Back, t0 + ms(200)), Some(Action::Back));
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn long_back_fires_home_once() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        d.press(&1, Action::Back, t0);
        assert_eq!(run_until(&mut d, t0, ms(2000)), vec![(LONG_PRESS, Action::Home)]);
        assert_eq!(d.release(&1, Action::Back, t0 + ms(2000)), None);
    }

    #[test]
    fn unplugging_stops_repeat_and_long_press() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        d.press(&1, Action::Left, t0);
        d.press(&2, Action::Back, t0);
        d.release_device(&1);
        d.release_device(&2);
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn other_buttons_fire_on_press_only() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        for a in [Action::Select, Action::Home, Action::Menu, Action::PlayPause] {
            assert_eq!(d.press(&1, a, t0), Some(a));
            assert_eq!(d.release(&1, a, t0), None);
        }
        assert_eq!(d.deadline(), None);
    }
}
