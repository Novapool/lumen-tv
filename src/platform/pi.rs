// Raspberry Pi 5 implementations of the platform traits.

use std::process::Command;

use std::sync::Mutex;

use super::{KeyOverride, Tv};

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
        let remap = match apply_desired() {
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

    fn set_key_overrides(&self, label: &str, overrides: &[KeyOverride]) {
        if let Ok(mut d) = DESIRED.lock() {
            d.label = label.to_string();
            d.overrides = overrides.to_vec();
        }
    }

    fn apply_keymap(&self) -> Result<String, String> {
        apply_desired()
    }
}

/// One kernel keymap change on the CEC remote: the rc-cec scancode is the CEC
/// "User Control" code.
#[derive(Clone, Debug, PartialEq, Eq)]
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

// More input-event-codes.h values: the rc-cec defaults, read from the untouched
// keymap (rc1) on pitv 2026-10-24. Arrows 103/108/105/106, 0x44 = PLAYCD,
// 0x46 = PAUSECD.
const KEY_UP: u16 = 103;
const KEY_LEFT: u16 = 105;
const KEY_RIGHT: u16 = 106;
const KEY_DOWN: u16 = 108;
const KEY_PAUSECD: u16 = 201;
const KEY_PLAYCD: u16 = 200;

/// Every CEC remote button a per-app override can change: scancode (the CEC
/// "User Control" code), the Lumen action, the key the box gives it normally and
/// the key rc-cec starts with (`from`; only used for a log warning).
struct Button {
    scancode: u32,
    action: &'static str,
    default: u16,
    from: u16,
}

const BUTTONS: &[Button] = &[
    Button { scancode: 0x00, action: "Select", default: KEY_ENTER, from: KEY_OK },
    Button { scancode: 0x01, action: "Up", default: KEY_UP, from: KEY_UP },
    Button { scancode: 0x02, action: "Down", default: KEY_DOWN, from: KEY_DOWN },
    Button { scancode: 0x03, action: "Left", default: KEY_LEFT, from: KEY_LEFT },
    Button { scancode: 0x04, action: "Right", default: KEY_RIGHT, from: KEY_RIGHT },
    Button { scancode: 0x0d, action: "Back", default: KEY_BACK, from: KEY_EXIT },
    Button { scancode: 0x44, action: "PlayPause", default: KEY_PLAYCD, from: KEY_PLAYCD },
    Button { scancode: 0x46, action: "PlayPause", default: KEY_PAUSECD, from: KEY_PAUSECD },
];

/// The keymap writes for these overrides: every button, so a restart or a
/// crash that left a previous app's overrides in the kernel keymap is undone
/// too (`remap_remote` skips entries that are already right). Phase 0 (LG basic
/// remote): OK arrives as KEY_OK and Back as KEY_EXIT; Jellyfin in Chromium only
/// reacts to Enter and KEY_BACK, so those two are always rewritten. Home and
/// Menu are not in `BUTTONS`, so an override for them is ignored.
fn plan(overrides: &[KeyOverride]) -> Vec<Remap> {
    BUTTONS
        .iter()
        .map(|b| {
            let over = overrides.iter().rev().find(|o| o.action == b.action);
            Remap { scancode: b.scancode, name: b.action, from: b.from, to: over.map_or(b.default, |o| o.code) }
        })
        .collect()
}

#[derive(Default)]
struct Desired {
    label: String,
    overrides: Vec<KeyOverride>,
}

static DESIRED: Mutex<Desired> = Mutex::new(Desired { label: String::new(), overrides: Vec::new() });
/// Serializes keymap writes: the claim's and a foreground change's run on
/// different threads.
static APPLY: Mutex<()> = Mutex::new(());

/// Writes the *current* desired overrides (read under the write lock, not
/// captured earlier), so whichever write runs last leaves the newest state.
fn apply_desired() -> Result<String, String> {
    let _w = APPLY.lock().unwrap_or_else(|e| e.into_inner());
    let (label, overrides) = {
        let d = DESIRED.lock().unwrap_or_else(|e| e.into_inner());
        (d.label.clone(), d.overrides.clone())
    };
    let res = remap_remote(&plan(&overrides));
    if res.is_ok() && !overrides.is_empty() {
        let list: Vec<String> = overrides.iter().map(|o| format!("{}->{}", o.action, o.key)).collect();
        println!("[tv] keys for {label}: {}", list.join(", "));
    }
    res
}

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

/// Rewrites the kernel keymap of the HDMI remote per `entries` (needs the input
/// group, not root). Idempotent. Returns what it did, e.g. "OK 352->28,
/// Back already 158". Logs scancodes that hold neither `from` nor `to`.
#[cfg(target_os = "linux")]
fn remap_remote(entries: &[Remap]) -> Result<String, String> {
    use std::os::fd::AsRawFd;
    let path = find_remote_node()?;
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let fd = file.as_raw_fd();
    let mut done = Vec::new();
    for r in entries {
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
fn remap_remote(_entries: &[Remap]) -> Result<String, String> {
    Err("keymap remap is Linux only".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ov(action: &'static str, key: &'static str, code: u16) -> KeyOverride {
        KeyOverride { action, key, code }
    }

    fn to(plan: &[Remap], scancode: u32) -> Option<u16> {
        plan.iter().find(|r| r.scancode == scancode).map(|r| r.to)
    }

    #[test]
    fn no_overrides_plan_writes_every_default() {
        let p = plan(&[]);
        assert_eq!(p.len(), BUTTONS.len());
        assert_eq!((to(&p, 0x00), to(&p, 0x0d)), (Some(28), Some(158)));
    }

    #[test]
    fn override_changes_only_its_scancode() {
        let p = plan(&[ov("Back", "KEY_ESC", 1)]);
        assert_eq!(to(&p, 0x0d), Some(1));
        assert_eq!(to(&p, 0x00), Some(KEY_ENTER));
        assert_eq!(p.len(), BUTTONS.len());
    }

    #[test]
    fn playpause_override_covers_both_cec_codes() {
        let p = plan(&[ov("PlayPause", "KEY_K", 37)]);
        assert_eq!((to(&p, 0x44), to(&p, 0x46)), (Some(37), Some(37)));
        let p = plan(&[ov("Up", "KEY_K", 37)]);
        assert_eq!(to(&p, 0x01), Some(37));
    }

    #[test]
    fn defaults_match_the_pitv_keymap() {
        let p = plan(&[]);
        assert_eq!(to(&p, 0x01), Some(103));
        assert_eq!(to(&p, 0x44), Some(200));
        assert_eq!(to(&p, 0x46), Some(201));
    }

    #[test]
    fn home_and_menu_have_no_scancode() {
        let p = plan(&[ov("Home", "KEY_ESC", 1), ov("Menu", "KEY_ESC", 1)]);
        assert!(p.iter().all(|r| r.name != "Home" && r.name != "Menu"));
        assert_eq!(p, plan(&[]));
    }

    #[test]
    fn buttons_have_unique_scancodes() {
        for (i, b) in BUTTONS.iter().enumerate() {
            assert!(BUTTONS[i + 1..].iter().all(|o| o.scancode != b.scancode));
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
