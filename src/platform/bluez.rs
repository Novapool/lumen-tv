// BlueZ over `busctl --system` subprocesses (no D-Bus crate), plus one held
// `bluetoothctl --timeout N scan on` child for discovery: BlueZ ends a
// discovery session when the D-Bus client that started it disconnects, so a
// one-shot StartDiscovery call would do nothing. With no agent registered,
// Pair uses Just Works. Errors are returned as busctl's raw text; callers
// map them with `bluetooth::friendly_error`.

use std::process::{Command, Stdio};
use std::time::Duration;

use crate::bluetooth::{self, BtErr};
use super::scan_slot::ScanSlot;
use super::{Bluetooth, BtSnapshot};

const SERVICE: &str = "org.bluez";
const ADAPTER_PATH: &str = "/org/bluez/hci0";
const POWER_SUPPLY: &str = "/sys/class/power_supply";
/// How long a fresh scan child gets to fail before start_scan calls it a success.
const SCAN_SETTLE: Duration = Duration::from_millis(300);

#[derive(Default)]
pub struct BluezBt {
    scan: ScanSlot,
}

/// `/org/bluez/hci0/dev_AA_BB_CC_DD_EE_FF`; rejects anything that is not a
/// MAC so an address can never smuggle an argument.
fn device_path(addr: &str) -> Result<String, String> {
    let ok = addr.len() == 17
        && addr
            .split(':')
            .all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit()));
    if !ok || addr.split(':').count() != 6 {
        return Err(format!("bad Bluetooth address: {addr}"));
    }
    Ok(format!("{ADAPTER_PATH}/dev_{}", addr.to_uppercase().replace(':', "_")))
}

fn busctl(args: &[&str]) -> Result<String, String> {
    let mut cmd = Command::new("busctl");
    cmd.arg("--system").args(args).stdin(Stdio::null());
    crate::apps::drop_ambient_caps(&mut cmd);
    let out = cmd.output().map_err(|e| format!("busctl: {e}"))?;
    if !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.trim().to_string());
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Runs a Device1/Adapter1 call and logs it.
fn call(path: &str, iface: &str, method: &str, extra: &[&str]) -> Result<(), String> {
    let mut args = vec!["--timeout=60", "call", SERVICE, path, iface, method];
    args.extend_from_slice(extra);
    let r = busctl(&args).map(|_| ());
    match &r {
        Ok(()) => eprintln!("[bt] {method} {path}: ok"),
        Err(e) => eprintln!("[bt] {method} {path}: {e}"),
    }
    r
}

fn set_bool(path: &str, iface: &str, prop: &str, v: bool) -> Result<(), String> {
    let r = busctl(&["set-property", SERVICE, path, iface, prop, "b", if v { "true" } else { "false" }]).map(|_| ());
    match &r {
        Ok(()) => eprintln!("[bt] set {prop}={v} {path}: ok"),
        Err(e) => eprintln!("[bt] set {prop}={v} {path}: {e}"),
    }
    r
}

/// Best effort: is the device already paired? (false on any error)
fn is_paired(path: &str) -> bool {
    busctl(&["get-property", SERVICE, path, "org.bluez.Device1", "Paired"]).is_ok_and(|o| o.trim() == "b true")
}

/// `busctl --json=short get-property` for the named props on one interface;
/// one value slot per name when the call succeeds.
fn get_props(path: &str, iface: &str, names: &[&str]) -> Option<Vec<Option<serde_json::Value>>> {
    let mut args = vec!["--json=short", "get-property", SERVICE, path, iface];
    args.extend_from_slice(names);
    let vals = bluetooth::parse_prop_lines(&busctl(&args).ok()?);
    (vals.len() == names.len()).then_some(vals)
}

/// Required Device1 props come in one call; RSSI and Icon (absent on many
/// devices, which fails the call) in one more, then one by one on failure.
/// Unnamed devices are dropped by the panel anyway, so they skip the extras.
fn read_device(path: &str) -> Option<crate::platform::BtDeviceInfo> {
    const DEV: &str = "org.bluez.Device1";
    let mut names = vec!["Address", "Alias", "Paired", "Trusted", "Connected"];
    let mut vals = get_props(path, DEV, &names)?;
    let named = bluetooth::device_from_props(&names, &vals, None)?.name.is_some();
    if !named {
        return bluetooth::device_from_props(&names, &vals, None);
    }
    let opt = ["RSSI", "Icon"];
    if let Some(v) = get_props(path, DEV, &opt) {
        names.extend(opt);
        vals.extend(v);
    } else {
        for o in opt {
            if let Some(v) = get_props(path, DEV, &[o]) {
                names.push(o);
                vals.extend(v);
            }
        }
    }
    let connected = bluetooth::device_from_props(&names, &vals, None)?.connected;
    // Best effort: Battery1 exists only on some connected devices.
    let battery = connected
        .then(|| get_props(path, "org.bluez.Battery1", &["Percentage"]))
        .flatten()
        .and_then(|v| v.into_iter().next().flatten())
        .and_then(|v| v.as_i64());
    bluetooth::device_from_props(&names, &vals, battery)
}

/// Object paths from `tree`, then only the properties the panel needs (see
/// bluetooth.rs "per-property reads" for why not GetManagedObjects).
fn read_snapshot() -> Result<BtSnapshot, String> {
    let tree = busctl(&["--list", "tree", SERVICE])?;
    let (adapter_path, device_paths) = bluetooth::parse_bluez_paths(&tree);
    let mut snap = BtSnapshot::default();
    if let Some(p) = adapter_path {
        let names = ["Address", "Powered", "Discovering"];
        if let Some(vals) = get_props(&p, "org.bluez.Adapter1", &names) {
            snap.adapter = Some(bluetooth::adapter_from_props(&names, &vals));
        }
    }
    snap.devices = device_paths.iter().filter_map(|p| read_device(p)).collect();
    Ok(snap)
}

impl Bluetooth for BluezBt {
    fn snapshot(&self) -> Result<BtSnapshot, String> {
        let mut snap = read_snapshot()?;
        snap.scan_error = self.scan.take_error();
        // Classic-BT HID pads (DualSense) have no Battery1; hid-playstation
        // exposes a power_supply instead.
        let entries: Vec<String> = std::fs::read_dir(POWER_SUPPLY)
            .map(|rd| rd.flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect())
            .unwrap_or_default();
        crate::bluetooth::fill_battery_fallback(&mut snap.devices, &entries, |e| {
            std::fs::read_to_string(format!("{POWER_SUPPLY}/{e}/capacity")).ok()?.trim().parse().ok()
        });
        Ok(snap)
    }

    fn set_powered(&self, on: bool) -> Result<(), String> {
        eprintln!("[bt] power {}", if on { "on" } else { "off" });
        if !on {
            self.scan.stop();
        }
        set_bool(ADAPTER_PATH, "org.bluez.Adapter1", "Powered", on)
    }

    fn start_scan(&self, secs: u32) -> Result<(), String> {
        eprintln!("[bt] scan on ({secs} s)");
        let mut cmd = Command::new("bluetoothctl");
        cmd.args(["--timeout", &secs.to_string(), "scan", "on"]);
        crate::apps::drop_ambient_caps(&mut cmd);
        let r = self.scan.start(cmd, secs, SCAN_SETTLE);
        if let Err(e) = &r {
            eprintln!("[bt] scan failed to start: {e}");
        }
        r
    }

    fn stop_scan(&self) -> Result<(), String> {
        eprintln!("[bt] scan off");
        self.scan.stop();
        Ok(())
    }

    fn pair(&self, addr: &str) -> Result<(), String> {
        let path = device_path(addr)?;
        eprintln!("[bt] pair {addr}: trust, pair, connect");
        self.scan.stop(); // discovery shares the radio: pairing/connecting during it fails
        let was_paired = is_paired(&path);
        // Trusted first, so the pad's own HID connection right after pairing
        // is authorized without an agent, and it can reconnect by itself later.
        set_bool(&path, "org.bluez.Device1", "Trusted", true)?;
        match call(&path, "org.bluez.Device1", "Pair", &[]) {
            Ok(()) => {}
            Err(e) if bluetooth::classify(&e) == BtErr::AlreadyPaired => {}
            Err(e) => {
                if !was_paired {
                    // Don't leave a failed pairing trusted (best effort).
                    let _ = set_bool(&path, "org.bluez.Device1", "Trusted", false);
                }
                return Err(e);
            }
        }
        match call(&path, "org.bluez.Device1", "Connect", &[]) {
            // The pad often connects by itself right after Pair.
            Err(e) if bluetooth::classify(&e) == BtErr::AlreadyConnected => Ok(()),
            r => r,
        }
    }

    fn connect(&self, addr: &str) -> Result<(), String> {
        eprintln!("[bt] connect {addr}");
        let path = device_path(addr)?;
        self.scan.stop(); // discovery shares the radio: pairing/connecting during it fails
        call(&path, "org.bluez.Device1", "Connect", &[])
    }

    fn disconnect(&self, addr: &str) -> Result<(), String> {
        eprintln!("[bt] disconnect {addr}");
        call(&device_path(addr)?, "org.bluez.Device1", "Disconnect", &[])
    }

    fn forget(&self, addr: &str) -> Result<(), String> {
        let path = device_path(addr)?;
        eprintln!("[bt] forget {addr}");
        call(ADAPTER_PATH, "org.bluez.Adapter1", "RemoveDevice", &["o", &path])
    }
}

#[cfg(test)]
mod tests {
    use super::device_path;

    #[test]
    fn device_paths() {
        assert_eq!(device_path("aa:bb:CC:00:00:01").unwrap(), "/org/bluez/hci0/dev_AA_BB_CC_00_00_01");
        assert!(device_path("AA:BB:CC:00:00").is_err());
        assert!(device_path("AA:BB:CC:00:00:0g").is_err());
        assert!(device_path("AA:BB:CC:00:00:01 --x").is_err());
    }

    #[test]
    fn device_path_rejects_injection_shaped_input() {
        for bad in [
            "",
            "../../../etc/passwd",
            "AA:BB:CC:00:00:01/../x",
            "AA:BB:CC:00:00:01 ",
            " AA:BB:CC:00:00:01",
            "AA:BB:CC:00:00:01\n",
            "--help",
            "-- AA:BB:CC:00:00:",
            "AA-BB-CC-00-00-01",
            "AA:BB:CC:00:00:01:02",
            "AAA:BB:CC:00:0:01:0",
            "AA:BB:CC:00:00:0\u{e9}",
            "AA:BB:CC:00:00:+1",
        ] {
            assert!(device_path(bad).is_err(), "{bad:?}");
        }
    }
}
