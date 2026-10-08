// Hardware- and OS-specific code lives behind traits in this module so an
// x86 port swaps modules, not code. More traits arrive with the features that
// need them: Windows in M3, Installer in M4, Display in M5.

pub mod pi;

/// The TV the box is plugged into.
pub trait Tv {
    /// Registers with the TV over HDMI-CEC so its remote's buttons reach the
    /// box, and switches the TV to this input.
    fn claim_remote(&self) -> Result<(), String>;
}
