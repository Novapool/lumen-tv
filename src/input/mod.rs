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

use serde::Deserialize;

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
