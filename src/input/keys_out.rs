// Controller in web apps. Chromium (Jellyfin) ignores the pad, so while a
// `type = "url"` app is in front, gamepad actions become keyboard keys on a
// uinput virtual keyboard, the same way the TV remote's keys reach it.
//
// `app_key` is the pure decision; `Out` (Linux) owns the uinput device.

use super::dispatch::SourceClass;
use super::Action;
use crate::platform::KeyOverride;
use std::collections::BTreeMap;

/// The virtual keyboard's name. Lumen's own reader must skip it, or
/// keyboard.toml would match it and the keys would loop back as actions.
pub const DEVICE_NAME: &str = "lumen-keys";

pub fn is_own_device(name: &str) -> bool {
    name == DEVICE_NAME
}

// linux/input-event-codes.h
const KEY_ENTER: u16 = 28;
const KEY_UP: u16 = 103;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;
const KEY_PLAYPAUSE: u16 = 164;
const KEY_BACK: u16 = 158;

/// The keys a per-app `keys` override may name. Every code here is registered
/// on the uinput device (`build()` iterates this table): uinput drops keys the
/// device wasn't created with.
pub const KEY_TABLE: &[(&str, u16)] = &[
    ("KEY_ESC", 1),
    ("KEY_BACKSPACE", 14),
    ("KEY_TAB", 15),
    ("KEY_ENTER", KEY_ENTER),
    ("KEY_F", 33),
    ("KEY_J", 36),
    ("KEY_K", 37),
    ("KEY_L", 38),
    ("KEY_M", 50),
    ("KEY_SPACE", 57),
    ("KEY_UP", KEY_UP),
    ("KEY_LEFT", KEY_LEFT),
    ("KEY_RIGHT", KEY_RIGHT),
    ("KEY_DOWN", KEY_DOWN),
    ("KEY_BACK", KEY_BACK),
    ("KEY_PLAYPAUSE", KEY_PLAYPAUSE),
];

/// Name to kernel code, `None` for a name outside `KEY_TABLE`.
pub fn key_code(name: &str) -> Option<(&'static str, u16)> {
    KEY_TABLE.iter().find(|(n, _)| *n == name).copied()
}

/// Turns an entry's `keys` table into resolved overrides. Home/Menu and any
/// unknown action are dropped; an unknown key name is logged once per name and
/// that action keeps its default.
pub fn resolve_overrides(keys: Option<&BTreeMap<String, String>>) -> Vec<KeyOverride> {
    let Some(keys) = keys else { return Vec::new() };
    let mut out = Vec::new();
    for (action, key) in keys {
        let Some(action) = crate::registry::KEY_ACTIONS.into_iter().find(|a| a == action) else {
            continue;
        };
        match key_code(key) {
            Some((key, code)) => out.push(KeyOverride { action, key, code }),
            None => warn_unknown_key(key),
        }
    }
    out
}

fn warn_unknown_key(name: &str) {
    use std::sync::Mutex;
    static SEEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
    if let Ok(mut seen) = SEEN.lock() {
        if !seen.iter().any(|s| s == name) {
            seen.push(name.to_string());
            eprintln!("[input] keys: unknown key name {name:?} (known: {}); using the default", KEY_TABLE.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", "));
        }
    }
}

/// The key (name, code) a web app should get for this action, or None.
/// Only gamepad actions, only with a web app (`type = "url"`) in front.
/// Home is Lumen's; Menu has no Jellyfin key. `overrides` are the foreground
/// app's `keys`; an action without one gets the default.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn app_key(class: SourceClass, action: Action, fg_is_web_app: bool, overrides: &[KeyOverride]) -> Option<(&'static str, u16)> {
    if class != SourceClass::Gamepad || !fg_is_web_app {
        return None;
    }
    let (name, key, code) = match action {
        Action::Up => ("Up", "KEY_UP", KEY_UP),
        Action::Down => ("Down", "KEY_DOWN", KEY_DOWN),
        Action::Left => ("Left", "KEY_LEFT", KEY_LEFT),
        Action::Right => ("Right", "KEY_RIGHT", KEY_RIGHT),
        Action::Select => ("Select", "KEY_ENTER", KEY_ENTER),
        Action::Back => ("Back", "KEY_BACK", KEY_BACK),
        Action::PlayPause => ("PlayPause", "KEY_PLAYPAUSE", KEY_PLAYPAUSE),
        Action::Home | Action::Menu => return None,
    };
    Some(overrides.iter().find(|o| o.action == name).map_or((key, code), |o| (o.key, o.code)))
}

#[cfg(target_os = "linux")]
pub use linux::Out;

#[cfg(target_os = "linux")]
mod linux {
    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode};

    use super::DEVICE_NAME;

    /// The virtual keyboard; `None` inside when /dev/uinput can't be opened.
    pub struct Out(Option<VirtualDevice>);

    impl Out {
        /// Opens uinput once; logs once on failure and then does nothing.
        pub fn open() -> Out {
            match build() {
                Ok(dev) => {
                    println!("[input] app keys: virtual keyboard \"{DEVICE_NAME}\" ready");
                    Out(Some(dev))
                }
                Err(e) => {
                    eprintln!("[input] app keys disabled: cannot open /dev/uinput ({e})");
                    Out(None)
                }
            }
        }

        /// Taps the key: press, then release, as two frames.
        pub fn tap(&mut self, code: u16) {
            let Some(dev) = self.0.as_mut() else { return };
            for value in [1, 0] {
                if dev.emit(&[InputEvent::new(EventType::KEY.0, code, value)]).is_err() {
                    return;
                }
            }
        }
    }

    fn build() -> std::io::Result<VirtualDevice> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for (_, code) in super::KEY_TABLE {
            keys.insert(KeyCode::new(*code));
        }
        VirtualDevice::builder()?
            .name(DEVICE_NAME)
            .input_id(InputId::new(BusType::BUS_USB, 0x1209, 0x0002, 1))
            .with_keys(&keys)?
            .build()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use SourceClass::{Gamepad, Other};

    #[test]
    fn gamepad_in_web_app_table() {
        let k = |a| app_key(Gamepad, a, true, &[]).map(|(n, _)| n);
        assert_eq!(k(Action::Up), Some("KEY_UP"));
        assert_eq!(k(Action::Down), Some("KEY_DOWN"));
        assert_eq!(k(Action::Left), Some("KEY_LEFT"));
        assert_eq!(k(Action::Right), Some("KEY_RIGHT"));
        assert_eq!(k(Action::Select), Some("KEY_ENTER"));
        assert_eq!(k(Action::Back), Some("KEY_BACK"));
        assert_eq!(k(Action::PlayPause), Some("KEY_PLAYPAUSE"));
        assert_eq!(k(Action::Home), None);
        assert_eq!(k(Action::Menu), None);
    }

    #[test]
    fn codes_match_kernel() {
        assert_eq!(app_key(Gamepad, Action::Select, true, &[]).unwrap().1, 28);
        assert_eq!(app_key(Gamepad, Action::Back, true, &[]).unwrap().1, 158);
        assert_eq!(app_key(Gamepad, Action::Up, true, &[]).unwrap().1, 103);
    }

    #[test]
    fn nothing_for_other_sources_or_non_web_foreground() {
        for a in [Action::Up, Action::Select, Action::Back, Action::PlayPause, Action::Home, Action::Menu] {
            assert_eq!(app_key(Other, a, true, &[]), None, "keyboard/remote never injected");
            assert_eq!(app_key(Gamepad, a, false, &[]), None, "command apps / Lumen get nothing");
            assert_eq!(app_key(Other, a, false, &[]), None);
        }
    }

    fn esc_back() -> Vec<KeyOverride> {
        let keys = BTreeMap::from([("Back".to_string(), "KEY_ESC".to_string())]);
        resolve_overrides(Some(&keys))
    }

    #[test]
    fn names_resolve_to_kernel_codes() {
        for (n, c) in [("KEY_ESC", 1), ("KEY_BACKSPACE", 14), ("KEY_ENTER", 28), ("KEY_SPACE", 57), ("KEY_K", 37), ("KEY_BACK", 158), ("KEY_PLAYPAUSE", 164), ("KEY_UP", 103), ("KEY_DOWN", 108), ("KEY_LEFT", 105), ("KEY_RIGHT", 106)] {
            assert_eq!(key_code(n), Some((n, c)));
        }
        assert_eq!(key_code("KEY_NOPE"), None);
        assert_eq!(key_code("key_esc"), None);
    }

    #[test]
    fn key_table_has_no_duplicates() {
        for (i, (n, c)) in KEY_TABLE.iter().enumerate() {
            assert!(KEY_TABLE[i + 1..].iter().all(|(n2, c2)| n2 != n && c2 != c), "{n} {c}");
        }
    }

    #[test]
    fn override_replaces_only_its_action() {
        let o = esc_back();
        assert_eq!(app_key(Gamepad, Action::Back, true, &o), Some(("KEY_ESC", 1)));
        assert_eq!(app_key(Gamepad, Action::Select, true, &o), Some(("KEY_ENTER", 28)));
        assert_eq!(app_key(Gamepad, Action::Up, true, &o), Some(("KEY_UP", 103)));
    }

    #[test]
    fn override_still_needs_gamepad_and_web_app() {
        let o = esc_back();
        assert_eq!(app_key(Other, Action::Back, true, &o), None);
        assert_eq!(app_key(Gamepad, Action::Back, false, &o), None);
        assert_eq!(app_key(Gamepad, Action::Home, true, &o), None);
    }

    #[test]
    fn home_menu_and_unknown_entries_are_ignored() {
        let keys = BTreeMap::from([
            ("Home".to_string(), "KEY_ESC".to_string()),
            ("Menu".to_string(), "KEY_ESC".to_string()),
            ("Bogus".to_string(), "KEY_ESC".to_string()),
            ("Select".to_string(), "KEY_NOPE".to_string()),
            ("PlayPause".to_string(), "KEY_K".to_string()),
        ]);
        let o = resolve_overrides(Some(&keys));
        assert_eq!(o, vec![KeyOverride { action: "PlayPause", key: "KEY_K", code: 37 }]);
        assert_eq!(app_key(Gamepad, Action::Select, true, &o), Some(("KEY_ENTER", 28)), "unknown name keeps default");
        assert!(resolve_overrides(None).is_empty());
    }

    #[test]
    fn own_device_is_excluded_but_press_is_not() {
        assert!(is_own_device("lumen-keys"));
        assert!(!is_own_device("lumen-press"));
        assert!(!is_own_device("Logitech K400"));
    }
}
