// The input layer. Reads every input device straight from evdev (never
// grabbing it, so other apps still get raw input), maps raw codes to actions
// with per-device TOML files (config/input/), and delivers normalized actions
// with hold-to-repeat and long-press Back → Home.
//
//   devices.rs   Linux: find devices, follow hotplug, one reader thread each
//   mapping.rs   which file a device uses; raw key/axis events → presses
//   dispatch.rs  presses/releases → actions (repeat, long-press)
//   keys_out.rs  gamepad actions → keyboard keys for web apps (uinput)

mod dispatch;
mod keys_out;
mod mapping;
pub use mapping::SHIPPED;

#[cfg(target_os = "linux")]
mod devices;

#[cfg(target_os = "linux")]
pub use devices::start;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Deserialize;

static FOREGROUND: Mutex<Option<String>> = Mutex::new(None);

/// The registry id of the app in front (None = Lumen), kept current by the
/// session for the held-PS rule on the input thread.
pub fn set_foreground(id: Option<String>) {
    if let Ok(mut f) = FOREGROUND.lock() {
        *f = id;
    }
}

static FOREGROUND_WEB: AtomicBool = AtomicBool::new(false);

/// Whether the app in front is a `type = "url"` web app (gets pad keys).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn set_foreground_web(web: bool) {
    FOREGROUND_WEB.store(web, Ordering::Relaxed);
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn foreground_is_web() -> bool {
    FOREGROUND_WEB.load(Ordering::Relaxed)
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn foreground() -> Option<String> {
    FOREGROUND.lock().ok().and_then(|f| f.clone())
}

/// What Settings shows about one open input device.
#[derive(Clone, Debug)]
pub struct DeviceStatus {
    pub path: String,
    pub name: String,
    /// The mapping file in use, or `None` when no mapping matched (ignored).
    pub mapping: Option<String>,
    pub last_action: Option<(Action, Instant)>,
    /// Last raw key name, recorded only while input debug is on.
    pub last_raw: Option<(String, Instant)>,
}

/// Shared between the input thread (writer) and the Settings feed (reader).
pub type Status = Arc<Mutex<Vec<DeviceStatus>>>;

/// The normalized events the UI understands (BRIEF §4).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    Up,
    Down,
    Left,
    Right,
    Select,
    Back,
    Home,
    Menu,
    PlayPause,
}

impl Action {
    fn is_direction(self) -> bool {
        matches!(self, Action::Up | Action::Down | Action::Left | Action::Right)
    }
}
