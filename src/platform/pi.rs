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
    fn claim_remote(&self) -> Result<String, String> {
        // As a Playback device (default vendor ID: an LG vendor ID forwarded
        // nothing extra in Phase 0) the TV sends remote presses ("User
        // Control") to us, and the kernel turns them into key events on
        // vc4-hdmi-0.
        let status = self.cec_ctl(&["--playback", "--osd-name", "Lumen"])?;
        let phys = status
            .lines()
            .find_map(|l| l.trim().strip_prefix("Physical Address").map(|r| r.trim_start_matches([' ', ':']).trim().to_string()))
            .filter(|p| p != "f.f.f.f")
            .ok_or("no HDMI physical address (TV off or unplugged?)")?;
        self.cec_ctl(&["--active-source", &format!("phys-addr={phys}")])?;
        // The remap is best effort: the claim stands without it, but the log
        // and the status string say so.
        let remap = match remap_remote() {
            Ok(done) => {
                println!("[input] remote keymap: {done}");
                format!("remap ok ({done})")
            }
            Err(e) => {
                eprintln!("[input] remote keymap remap FAILED: {e}");
                format!("remap FAILED: {e}")
            }
        };
        let status = format!("Playback \"Lumen\" at {phys}, {remap}, {}", chrono::Local::now().format("%H:%M:%S"));
        println!("[input] remote claimed: {status}");
        Ok(status)
    }
}

/// One kernel keymap change on the CEC remote: the rc-cec scancode is the CEC
/// "User Control" code.
#[derive(Debug, PartialEq, Eq)]
pub struct Remap {
    pub scancode: u32,
    pub name: &'static str,
    pub from: u16,
    pub to: u16,
}

// Linux input-event-codes.h
const KEY_ENTER: u16 = 28;
const KEY_EXIT: u16 = 174;
const KEY_BACK: u16 = 158;
const KEY_OK: u16 = 352;

/// Phase 0 (LG basic remote): OK arrives as KEY_OK, Back as KEY_EXIT. Jellyfin
/// in Chromium only reacts to Enter and KEY_BACK, so rewrite both.
pub const REMAP: &[Remap] = &[
    Remap { scancode: 0x00, name: "OK", from: KEY_OK, to: KEY_ENTER },
    Remap { scancode: 0x0d, name: "Back", from: KEY_EXIT, to: KEY_BACK },
];

/// The vc4-hdmi input node, found by device name in sysfs.
#[cfg(target_os = "linux")]
fn find_remote_node() -> Result<std::path::PathBuf, String> {
    let dir = std::fs::read_dir("/sys/class/input").map_err(|e| format!("/sys/class/input: {e}"))?;
    for entry in dir.flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if !file.starts_with("event") {
            continue;
        }
        let name = std::fs::read_to_string(entry.path().join("device/name")).unwrap_or_default();
        // Exact name: "vc4-hdmi-0 HDMI Jack" and vc4-hdmi-1 also exist.
        if name.trim() == "vc4-hdmi-0" {
            return Ok(std::path::PathBuf::from("/dev/input").join(file));
        }
    }
    Err("no vc4-hdmi input node".into())
}

/// `struct input_keymap_entry` (linux/input.h).
#[cfg(target_os = "linux")]
#[repr(C)]
#[derive(Default)]
struct KeymapEntry {
    flags: u8,
    len: u8,
    index: u16,
    keycode: u32,
    scancode: [u8; 32],
}

// _IOR/_IOW('E', 0x04, struct input_keymap_entry): EVIOCGKEYCODE_V2 / EVIOCSKEYCODE_V2.
#[cfg(target_os = "linux")]
const EVIOCGKEYCODE_V2: u64 = (2 << 30) | ((std::mem::size_of::<KeymapEntry>() as u64) << 16) | (0x45 << 8) | 0x04;
#[cfg(target_os = "linux")]
const EVIOCSKEYCODE_V2: u64 = (1 << 30) | ((std::mem::size_of::<KeymapEntry>() as u64) << 16) | (0x45 << 8) | 0x04;

/// Rewrites the kernel keymap of the HDMI remote per `REMAP` (needs the input
/// group, not root). Idempotent. Returns what it did, e.g. "OK->KEY_ENTER set,
/// Back->KEY_BACK already".
#[cfg(target_os = "linux")]
fn remap_remote() -> Result<String, String> {
    use std::os::fd::AsRawFd;
    let path = find_remote_node()?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let fd = file.as_raw_fd();
    let mut done = Vec::new();
    for r in REMAP {
        let mut e = KeymapEntry { len: 4, ..Default::default() };
        e.scancode[..4].copy_from_slice(&r.scancode.to_le_bytes());
        // SAFETY: fd is open, `e` is a valid repr(C) input_keymap_entry.
        if unsafe { libc::ioctl(fd, EVIOCGKEYCODE_V2 as _, &mut e as *mut KeymapEntry) } < 0 {
            return Err(format!("{} read scancode {:#x}: {}", path.display(), r.scancode, std::io::Error::last_os_error()));
        }
        let now = e.keycode;
        if now == r.to as u32 {
            done.push(format!("{} already {}", r.name, r.to));
            continue;
        }
        if now != r.from as u32 {
            eprintln!("[input] remote keymap: scancode {:#x} ({}) is key {now}, expected {}; remapping anyway", r.scancode, r.name, r.from);
        }
        e.keycode = r.to as u32;
        // SAFETY: as above.
        if unsafe { libc::ioctl(fd, EVIOCSKEYCODE_V2 as _, &e as *const KeymapEntry) } < 0 {
            return Err(format!("{} set scancode {:#x}: {}", path.display(), r.scancode, std::io::Error::last_os_error()));
        }
        done.push(format!("{} {now}->{}", r.name, r.to));
    }
    Ok(done.join(", "))
}

#[cfg(not(target_os = "linux"))]
fn remap_remote() -> Result<String, String> {
    Err("keymap remap is Linux only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remap_table_matches_phase_0() {
        assert_eq!(REMAP.len(), 2);
        assert_eq!((REMAP[0].scancode, REMAP[0].from, REMAP[0].to), (0x00, 352, 28));
        assert_eq!((REMAP[1].scancode, REMAP[1].from, REMAP[1].to), (0x0d, 174, 158));
    }

    #[test]
    fn remap_has_no_duplicate_scancodes_or_noops() {
        for (i, r) in REMAP.iter().enumerate() {
            assert_ne!(r.from, r.to);
            assert!(REMAP[i + 1..].iter().all(|o| o.scancode != r.scancode));
        }
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn ioctl_numbers_match_the_kernel() {
        assert_eq!(std::mem::size_of::<KeymapEntry>(), 40);
        assert_eq!(EVIOCGKEYCODE_V2, 0x8028_4504);
        assert_eq!(EVIOCSKEYCODE_V2, 0x4028_4504);
    }
}
