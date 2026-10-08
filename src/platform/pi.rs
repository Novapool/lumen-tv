// Raspberry Pi 5 implementations of the platform traits.

use std::process::Command;

use super::Tv;

/// The TV on HDMI0, reached through the kernel CEC adapter with `cec-ctl`
/// (v4l-utils). Kodi's libcec does the same when Kodi runs.
pub struct PiTv {
    pub device: &'static str,
}

impl Default for PiTv {
    fn default() -> Self {
        PiTv { device: "/dev/cec0" }
    }
}

impl PiTv {
    fn cec_ctl(&self, args: &[&str]) -> Result<String, String> {
        let out = Command::new("cec-ctl")
            .arg("-d")
            .arg(self.device)
            .args(args)
            .output()
            .map_err(|e| format!("cec-ctl: {e}"))?;
        if !out.status.success() {
            return Err(format!("cec-ctl {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()));
        }
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    }
}

impl Tv for PiTv {
    fn claim_remote(&self) -> Result<(), String> {
        // As a Playback device the TV sends remote presses ("User Control")
        // to us, and the kernel turns them into key events on vc4-hdmi-0.
        let status = self.cec_ctl(&["--playback", "--osd-name", "Lumen"])?;
        let phys = status
            .lines()
            .find_map(|l| l.trim().strip_prefix("Physical Address").map(|r| r.trim_start_matches([' ', ':']).trim().to_string()))
            .filter(|p| p != "f.f.f.f")
            .ok_or("no HDMI physical address (TV off or unplugged?)")?;
        self.cec_ctl(&["--active-source", &format!("phys-addr={phys}")])?;
        Ok(())
    }
}
