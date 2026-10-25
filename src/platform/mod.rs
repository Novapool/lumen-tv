// Hardware- and OS-specific code lives behind traits in this module so an
// x86 port swaps modules, not code. More traits arrive with the features that
// need them: Installer in M4, Display in M5. `Windows` (M3) is the window
// control seam; its Linux implementation talks to labwc in `toplevel`.

pub mod fake_bt;
pub mod output;
pub mod pi;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod scan_slot;
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
mod agent_slot;
#[cfg(target_os = "linux")]
pub mod bluez;
#[cfg(target_os = "linux")]
pub mod toplevel;

/// The TV the box is plugged into.
pub trait Tv {
    /// Registers with the TV over HDMI-CEC so its remote's buttons reach the
    /// box, switches the TV to this input and remaps the remote's keys for
    /// apps. Returns a one-line status (identity, remap result, time) for
    /// Settings; a failed remap is reported in the status, not as an error.
    fn claim_remote(&self) -> Result<String, String>;

    /// Records which keys the remote's buttons should produce while the app
    /// `label` is in front (empty = defaults). Cheap and non-blocking: call it
    /// on the UI thread, in order, then `apply_keymap` off-thread. The record is
    /// what every keymap write (this one and the claim's) applies, so the last
    /// call wins no matter which worker thread runs last.
    fn set_key_overrides(&self, label: &str, overrides: &[KeyOverride]);

    /// Writes the recorded overrides into the remote's keymap (blocking ioctls).
    /// Idempotent. Returns a one-line description of what it did.
    fn apply_keymap(&self) -> Result<String, String>;
}

/// One per-app key override: the Lumen action (`"Back"`, ...) now sends this
/// key (`"KEY_ESC"`, kernel code 1). Resolved by `input::keys_out`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyOverride {
    pub action: &'static str,
    pub key: &'static str,
    pub code: u16,
}

/// One top-level window of any app, as the compositor reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Toplevel {
    /// Lumen-assigned, stable for the window's life, never reused.
    pub id: u64,
    pub app_id: String,
    pub title: String,
    pub activated: bool,
    pub fullscreen: bool,
}

/// A change in the set of windows. Sent after the compositor's `done`, so the
/// record is complete.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowEvent {
    Opened(Toplevel),
    Changed(Toplevel),
    Closed(u64),
}

/// Looks at and controls other apps' windows.
pub trait Windows: Send + Sync {
    /// Current windows, ordered by id.
    fn toplevels(&self) -> Vec<Toplevel>;
    /// Brings the window to the front and gives it keyboard focus.
    fn activate(&self, id: u64) -> Result<(), String>;
    /// Asks the app to close the window (it may refuse or ask to save).
    fn close(&self, id: u64) -> Result<(), String>;
    fn set_fullscreen(&self, id: u64) -> Result<(), String>;
}

/// The Bluetooth adapter and its devices. Every call may block (BlueZ over
/// D-Bus), so callers use worker threads.
pub trait Bluetooth: Send + Sync {
    fn snapshot(&self) -> Result<BtSnapshot, String>;
    fn set_powered(&self, on: bool) -> Result<(), String>;
    /// Scans for up to `secs` seconds.
    fn start_scan(&self, secs: u32) -> Result<(), String>;
    fn stop_scan(&self) -> Result<(), String>;
    /// The whole sequence: trust, pair, connect.
    fn pair(&self, addr: &str) -> Result<(), String>;
    fn connect(&self, addr: &str) -> Result<(), String>;
    fn disconnect(&self, addr: &str) -> Result<(), String>;
    fn forget(&self, addr: &str) -> Result<(), String>;
    /// The panel closed: release what was held for it (the pairing agent).
    fn close(&self) {}
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtAdapter {
    pub address: String,
    pub powered: bool,
    pub discovering: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtDeviceInfo {
    pub address: String,
    pub name: Option<String>,
    pub icon: Option<String>,
    pub paired: bool,
    pub trusted: bool,
    pub connected: bool,
    pub rssi: Option<i16>,
    pub battery: Option<u8>,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct BtSnapshot {
    pub adapter: Option<BtAdapter>,
    pub devices: Vec<BtDeviceInfo>,
    /// Set once if the scan process died early (its stderr or exit status).
    pub scan_error: Option<String>,
}

/// The Bluetooth backend: the fake with `LUMEN_BT=fake` (and always off
/// Linux), BlueZ otherwise.
pub fn bluetooth() -> std::sync::Arc<dyn Bluetooth> {
    #[cfg(target_os = "linux")]
    if std::env::var("LUMEN_BT").map_or(true, |v| v != "fake") {
        return std::sync::Arc::new(bluez::BluezBt::default());
    }
    std::sync::Arc::new(fake_bt::FakeBt::default())
}
