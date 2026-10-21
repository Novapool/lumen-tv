// Controller in web apps. Chromium (Jellyfin) ignores the pad, so while a
// `type = "url"` app is in front, gamepad actions become keyboard keys on a
// uinput virtual keyboard, the same way the TV remote's keys reach it.
//
// `app_key` is the pure decision; `Out` (Linux) owns the uinput device.

use super::dispatch::SourceClass;
use super::Action;

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

/// The key (name, code) a web app should get for this action, or None.
/// Only gamepad actions, only with a web app (`type = "url"`) in front.
/// Home is Lumen's; Menu has no Jellyfin key.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn app_key(class: SourceClass, action: Action, fg_is_web_app: bool) -> Option<(&'static str, u16)> {
    if class != SourceClass::Gamepad || !fg_is_web_app {
        return None;
    }
    match action {
        Action::Up => Some(("KEY_UP", KEY_UP)),
        Action::Down => Some(("KEY_DOWN", KEY_DOWN)),
        Action::Left => Some(("KEY_LEFT", KEY_LEFT)),
        Action::Right => Some(("KEY_RIGHT", KEY_RIGHT)),
        Action::Select => Some(("KEY_ENTER", KEY_ENTER)),
        Action::Back => Some(("KEY_BACK", KEY_BACK)),
        Action::PlayPause => Some(("KEY_PLAYPAUSE", KEY_PLAYPAUSE)),
        Action::Home | Action::Menu => None,
    }
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
        for k in [
            KeyCode::KEY_UP,
            KeyCode::KEY_DOWN,
            KeyCode::KEY_LEFT,
            KeyCode::KEY_RIGHT,
            KeyCode::KEY_ENTER,
            KeyCode::KEY_BACK,
            KeyCode::KEY_PLAYPAUSE,
        ] {
            keys.insert(k);
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
        let k = |a| app_key(Gamepad, a, true).map(|(n, _)| n);
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
        assert_eq!(app_key(Gamepad, Action::Select, true).unwrap().1, 28);
        assert_eq!(app_key(Gamepad, Action::Back, true).unwrap().1, 158);
        assert_eq!(app_key(Gamepad, Action::Up, true).unwrap().1, 103);
    }

    #[test]
    fn nothing_for_other_sources_or_non_web_foreground() {
        for a in [Action::Up, Action::Select, Action::Back, Action::PlayPause, Action::Home, Action::Menu] {
            assert_eq!(app_key(Other, a, true), None, "keyboard/remote never injected");
            assert_eq!(app_key(Gamepad, a, false), None, "command apps / Lumen get nothing");
            assert_eq!(app_key(Other, a, false), None);
        }
    }

    #[test]
    fn own_device_is_excluded_but_press_is_not() {
        assert!(is_own_device("lumen-keys"));
        assert!(!is_own_device("lumen-press"));
        assert!(!is_own_device("Logitech K400"));
    }
}
