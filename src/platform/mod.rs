// Hardware- and OS-specific code lives behind traits in this module so an
// x86 port swaps modules, not code. More traits arrive with the features that
// need them: Installer in M4, Display in M5. `Windows` (M3) is the window
// control seam; its Linux implementation talks to labwc in `toplevel`.

pub mod pi;
#[cfg(target_os = "linux")]
pub mod toplevel;

/// The TV the box is plugged into.
pub trait Tv {
    /// Registers with the TV over HDMI-CEC so its remote's buttons reach the
    /// box, switches the TV to this input and remaps the remote's keys for
    /// apps. Returns a one-line status (identity, remap result, time) for
    /// Settings; a failed remap is reported in the status, not as an error.
    fn claim_remote(&self) -> Result<String, String>;
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
