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

/// Where a press came from, for the held-PS rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceClass {
    Gamepad,
    Other,
}

/// The app whose own PS button belongs to it (D4): a gamepad Home there needs
/// a hold.
const HELD_HOME_APP: &str = "steamlink";

/// Does a Home press from this source, with this app in front, have to be
/// held? Only a gamepad's Home button inside Steam Link. Keyboard F1 and the
/// remote's red are never held.
pub fn home_needs_hold(class: SourceClass, foreground: Option<&str>) -> bool {
    class == SourceClass::Gamepad && foreground == Some(HELD_HOME_APP)
}

/// Does a Back press from this source have the long-press-to-Home behaviour?
/// Not a gamepad's held Circle while any app is in front (`foreground` is the
/// app's id; None = Lumen's own screens): there it is plain Back for the app.
/// Keyboard and remote keep the long press everywhere.
pub fn back_long_press_is_home(class: SourceClass, foreground: Option<&str>) -> bool {
    !(class == SourceClass::Gamepad && foreground.is_some())
}

/// `S` identifies the device a press came from, so unplugging a device or
/// releasing on another one can't leave a direction stuck.
pub struct Dispatcher<S> {
    held: Option<Held<S>>,
    /// (source, since, long press turns it into Home).
    back: Option<(S, Instant, bool)>,
    /// A gamepad Home press that must be held (see `home_needs_hold`).
    home: Option<(S, Instant)>,
    held_home_fired: bool,
}

struct Held<S> {
    source: S,
    action: Action,
    next: Instant,
    repeats: i32,
}

impl<S: Clone + PartialEq> Default for Dispatcher<S> {
    fn default() -> Self {
        Dispatcher { held: None, back: None, home: None, held_home_fired: false }
    }
}

impl<S: Clone + PartialEq> Dispatcher<S> {
    #[cfg(test)]
    pub fn press(&mut self, source: &S, action: Action, now: Instant) -> Option<Action> {
        self.press_rules(source, action, now, false, true)
    }

    /// Like `press`; `hold_home` makes a Home press wait for LONG_PRESS (a
    /// release before that does nothing). `back_long_home` false makes a Back
    /// press plain: it fires on release however long it was held.
    pub fn press_rules(
        &mut self,
        source: &S,
        action: Action,
        now: Instant,
        hold_home: bool,
        back_long_home: bool,
    ) -> Option<Action> {
        if action == Action::Home && hold_home {
            self.home = Some((source.clone(), now));
            None
        } else if action.is_direction() {
            // The latest direction wins; it alone repeats.
            self.held = Some(Held { source: source.clone(), action, next: now + REPEAT_DELAY, repeats: 0 });
            Some(action)
        } else if action == Action::Back {
            self.back = Some((source.clone(), now, back_long_home));
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
                Some((s, since, long)) if s == *source && (!long || now - since < LONG_PRESS) => Some(Action::Back),
                other => {
                    self.back = other.filter(|(s, _, _)| s != source);
                    None
                }
            }
        } else if action == Action::Home {
            // A held-Home press released early: nothing (Steam gets the tap).
            if self.home.as_ref().is_some_and(|(s, _)| s == source) {
                self.home = None;
            }
            None
        } else {
            None
        }
    }

    /// Forgets everything a device was holding (it was unplugged).
    pub fn release_device(&mut self, source: &S) {
        if self.held.as_ref().is_some_and(|h| h.source == *source) {
            self.held = None;
        }
        if self.back.as_ref().is_some_and(|(s, _, _)| s == source) {
            self.back = None;
        }
        if self.home.as_ref().is_some_and(|(s, _)| s == source) {
            self.home = None;
        }
    }

    /// True once after tick() fired a held Home press (for the log line).
    pub fn take_held_home_fired(&mut self) -> bool {
        std::mem::take(&mut self.held_home_fired)
    }

    /// When tick() next has something to do.
    pub fn deadline(&self) -> Option<Instant> {
        let repeat = self.held.as_ref().map(|h| h.next);
        let long_press = self.back.as_ref().filter(|b| b.2).map(|(_, since, _)| *since + LONG_PRESS);
        let held_home = self.home.as_ref().map(|(_, since)| *since + LONG_PRESS);
        repeat.into_iter().chain(long_press).chain(held_home).min()
    }

    pub fn tick(&mut self, now: Instant) -> Vec<(S, Action)> {
        let mut out = Vec::new();
        if let Some(h) = self.held.as_mut().filter(|h| h.next <= now) {
            out.push((h.source.clone(), h.action));
            h.repeats += 1;
            let interval = REPEAT_START.mul_f64(REPEAT_ACCEL.powi(h.repeats - 1));
            h.next = now + interval.max(REPEAT_MIN);
        }
        if let Some((source, _, _)) = self.back.take_if(|(_, since, long)| *long && *since + LONG_PRESS <= now) {
            out.push((source, Action::Home));
        }
        if let Some((source, _)) = self.home.take_if(|(_, since)| *since + LONG_PRESS <= now) {
            self.held_home_fired = true;
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
    fn home_rule_table() {
        use SourceClass::{Gamepad, Other};
        // Outside Steam Link a tap is Home, from any source.
        for fg in [None, Some("jellyfin"), Some("kodi")] {
            assert!(!home_needs_hold(Gamepad, fg));
            assert!(!home_needs_hold(Other, fg));
        }
        // Inside Steam Link only a gamepad needs the hold.
        assert!(home_needs_hold(Gamepad, Some("steamlink")));
        assert!(!home_needs_hold(Other, Some("steamlink")), "F1 / red stay Home");
    }

    #[test]
    fn back_long_press_rule_table() {
        use SourceClass::{Gamepad, Other};
        // Lumen's own screens: every source keeps long Back = Home.
        assert!(back_long_press_is_home(Gamepad, None));
        assert!(back_long_press_is_home(Other, None));
        // An app in front: only the gamepad loses it.
        for fg in ["steamlink", "kodi", "jellyfin"] {
            assert!(!back_long_press_is_home(Gamepad, Some(fg)), "{fg}");
            assert!(back_long_press_is_home(Other, Some(fg)), "{fg}");
        }
    }

    #[test]
    fn plain_back_fires_on_release_however_long() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        assert_eq!(d.press_rules(&1, Action::Back, t0, false, false), None);
        assert_eq!(d.deadline(), None, "no long-press timer");
        assert!(run_until(&mut d, t0, ms(3000)).is_empty(), "never turns into Home");
        assert_eq!(d.release(&1, Action::Back, t0 + ms(3000)), Some(Action::Back));
        assert_eq!(d.release(&1, Action::Back, t0 + ms(3100)), None);
    }

    #[test]
    fn plain_back_is_per_press_not_sticky() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        d.press_rules(&1, Action::Back, t0, false, false);
        d.release(&1, Action::Back, t0 + ms(100));
        // The next press, with Lumen in front, long-presses to Home again.
        d.press_rules(&1, Action::Back, t0 + ms(200), false, true);
        assert_eq!(run_until(&mut d, t0, ms(2000)), vec![(ms(200) + LONG_PRESS, Action::Home)]);
    }

    #[test]
    fn held_ps_in_steamlink_fires_home_after_the_hold() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        let hold = home_needs_hold(SourceClass::Gamepad, Some("steamlink"));
        assert_eq!(d.press_rules(&1, Action::Home, t0, hold, true), None);
        assert_eq!(run_until(&mut d, t0, ms(2000)), vec![(LONG_PRESS, Action::Home)]);
        assert!(d.take_held_home_fired());
        assert!(!d.take_held_home_fired());
        assert_eq!(d.release(&1, Action::Home, t0 + ms(2000)), None);
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn ps_tap_in_steamlink_does_nothing() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        assert_eq!(d.press_rules(&1, Action::Home, t0, true, true), None);
        assert_eq!(d.release(&1, Action::Home, t0 + ms(300)), None);
        assert_eq!(d.deadline(), None);
        assert!(run_until(&mut d, t0, ms(2000)).is_empty());
    }

    #[test]
    fn ps_tap_elsewhere_is_home_on_press() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        let hold = home_needs_hold(SourceClass::Gamepad, Some("jellyfin"));
        assert_eq!(d.press_rules(&1, Action::Home, t0, hold, true), Some(Action::Home));
        assert_eq!(d.deadline(), None);
    }

    #[test]
    fn unplugging_cancels_a_held_ps() {
        let t0 = Instant::now();
        let mut d = Dispatcher::default();
        d.press_rules(&1, Action::Home, t0, true, true);
        d.release_device(&1);
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
