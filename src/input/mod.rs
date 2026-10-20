// The input layer. Reads every input device straight from evdev (never
// grabbing it, so other apps still get raw input), maps raw codes to actions
// with per-device TOML files (config/input/), and delivers normalized actions
// with hold-to-repeat and long-press Back → Home.
//
//   devices.rs   Linux: find devices, follow hotplug, one reader thread each
//   mapping.rs   which file a device uses; raw key/axis events → presses
//   dispatch.rs  presses/releases → actions (repeat, long-press)

mod dispatch;
mod mapping;

#[cfg(target_os = "linux")]
mod devices;

#[cfg(target_os = "linux")]
pub use devices::start;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use serde::Deserialize;

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
