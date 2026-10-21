//! Bluetooth logic for the Settings > Bluetooth panel: pure functions that
//! turn BlueZ's `GetManagedObjects` output into a `platform::BtSnapshot`, and
//! a snapshot into the UI's device rows and header state. No I/O here, so it
//! is all unit tested on the Mac. The backends live in `platform::bluez`
//! (Linux) and `platform::fake_bt`.

use std::time::{Duration, Instant};

use serde_json::Value;

use crate::platform::{BtAdapter, BtDeviceInfo, BtSnapshot};

/// D7: a scan stops by itself after this long.
pub const SCAN_SECS: u32 = 60;
/// D6: at most this many devices are listed.
pub const MAX_DEVICES: usize = 20;

// ---------- parsing ----------

/// Parses `busctl --system --json=short call org.bluez / \
/// org.freedesktop.DBus.ObjectManager GetManagedObjects`:
/// `{"type":"a{oa{sa{sv}}}","data":[{path:{iface:{prop:{"type":..,"data":..}}}}]}`.
/// Devices come out in object-path order, unfiltered and with `battery` taken
/// from `org.bluez.Battery1` when present.
#[cfg(test)]
pub fn parse_managed_objects(json: &str) -> Result<BtSnapshot, String> {
    let v: Value = serde_json::from_str(json).map_err(|e| format!("bluez json: {e}"))?;
    let objects = v
        .get("data")
        .and_then(|d| d.get(0))
        .and_then(Value::as_object)
        .ok_or("bluez json: no data[0] object")?;
    let mut snap = BtSnapshot::default();
    for (_path, ifaces) in objects {
        if let Some(a) = ifaces.get("org.bluez.Adapter1") {
            // The first adapter wins (the Pi has only hci0).
            if snap.adapter.is_none() {
                snap.adapter = Some(BtAdapter {
                    address: prop_str(a, "Address").unwrap_or_default(),
                    powered: prop_bool(a, "Powered").unwrap_or(false),
                    discovering: prop_bool(a, "Discovering").unwrap_or(false),
                });
            }
        }
        if let Some(d) = ifaces.get("org.bluez.Device1") {
            let Some(address) = prop_str(d, "Address") else { continue };
            let dashed = address.replace(':', "-");
            // Alias defaults to the dashed MAC when the device has no name.
            let name = ["Alias", "Name"]
                .iter()
                .filter_map(|k| prop_str(d, k))
                .find(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&dashed) && !n.eq_ignore_ascii_case(&address));
            snap.devices.push(BtDeviceInfo {
                name,
                icon: prop_str(d, "Icon"),
                paired: prop_bool(d, "Paired").unwrap_or(false),
                trusted: prop_bool(d, "Trusted").unwrap_or(false),
                connected: prop_bool(d, "Connected").unwrap_or(false),
                rssi: prop_i64(d, "RSSI").and_then(|n| i16::try_from(n).ok()),
                battery: ifaces
                    .get("org.bluez.Battery1")
                    .and_then(|b| prop_i64(b, "Percentage"))
                    .map(|n| n.clamp(0, 100) as u8),
                address,
            });
        }
    }
    Ok(snap)
}

// ---------- per-property reads ----------
//
// GetManagedObjects can't be used live: busctl's JSON writer fails on the
// whole reply when any device carries a property with non-string dict keys
// (ManufacturerData is `a{qv}`). So the backend lists object paths, then reads
// only the properties it needs, so one bad property can never sink a snapshot.

/// At most this many device paths are read per snapshot.
pub const MAX_DEVICE_PATHS: usize = 40;

/// `busctl --list tree org.bluez` -> (adapter path if present, device paths),
/// keeping only `/org/bluez/hciN` and `/org/bluez/hciN/dev_XX_XX_XX_XX_XX_XX`.
pub fn parse_bluez_paths(list: &str) -> (Option<String>, Vec<String>) {
    let mut adapter = None;
    let mut devices = Vec::new();
    for line in list.lines() {
        let Some(rest) = line.trim().strip_prefix("/org/bluez/hci") else { continue };
        let (n, tail) = rest.split_once('/').unwrap_or((rest, ""));
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        if tail.is_empty() {
            adapter.get_or_insert_with(|| line.trim().to_string());
        } else if let Some(mac) = tail.strip_prefix("dev_") {
            let parts: Vec<_> = mac.split('_').collect();
            if parts.len() == 6 && parts.iter().all(|p| p.len() == 2 && p.chars().all(|c| c.is_ascii_hexdigit())) {
                devices.push(line.trim().to_string());
            }
        }
    }
    devices.truncate(MAX_DEVICE_PATHS);
    (adapter, devices)
}

/// Output of `busctl --json=short get-property ... P1 P2 ...`: one
/// `{"type":..,"data":..}` per line, in order. A line that does not parse
/// gives `None` in its slot and leaves the others alone.
pub fn parse_prop_lines(out: &str) -> Vec<Option<Value>> {
    out.lines()
        .filter(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str::<Value>(l).ok().and_then(|v| v.get("data").cloned()))
        .collect()
}

fn pair_up<'a>(names: &[&'a str], vals: &[Option<Value>]) -> Vec<(&'a str, Value)> {
    names.iter().zip(vals).filter_map(|(n, v)| Some((*n, v.clone()?))).collect()
}

fn get<'a>(props: &'a [(&str, Value)], k: &str) -> Option<&'a Value> {
    props.iter().find(|(n, _)| *n == k).map(|(_, v)| v)
}

/// Adapter1 `Address Powered Discovering`.
pub fn adapter_from_props(names: &[&str], vals: &[Option<Value>]) -> BtAdapter {
    let p = pair_up(names, vals);
    BtAdapter {
        address: get(&p, "Address").and_then(Value::as_str).unwrap_or_default().to_string(),
        powered: get(&p, "Powered").and_then(Value::as_bool).unwrap_or(false),
        discovering: get(&p, "Discovering").and_then(Value::as_bool).unwrap_or(false),
    }
}

/// Device1 properties (any subset of Address Alias Name Paired Trusted
/// Connected RSSI Icon) plus an optional Battery1 percentage. None without
/// an Address. Alias/Name that is just the MAC is not a name.
pub fn device_from_props(names: &[&str], vals: &[Option<Value>], battery: Option<i64>) -> Option<BtDeviceInfo> {
    let p = pair_up(names, vals);
    let s = |k: &str| get(&p, k).and_then(Value::as_str).map(str::to_string);
    let b = |k: &str| get(&p, k).and_then(Value::as_bool).unwrap_or(false);
    let address = s("Address")?;
    let dashed = address.replace(':', "-");
    let name = ["Alias", "Name"]
        .iter()
        .filter_map(|k| s(k))
        .find(|n| !n.is_empty() && !n.eq_ignore_ascii_case(&dashed) && !n.eq_ignore_ascii_case(&address));
    Some(BtDeviceInfo {
        name,
        icon: s("Icon"),
        paired: b("Paired"),
        trusted: b("Trusted"),
        connected: b("Connected"),
        rssi: get(&p, "RSSI").and_then(Value::as_i64).and_then(|n| i16::try_from(n).ok()),
        battery: battery.map(|n| n.clamp(0, 100) as u8),
        address,
    })
}

#[cfg(test)]
fn prop<'a>(iface: &'a Value, key: &str) -> Option<&'a Value> {
    iface.get(key)?.get("data")
}
#[cfg(test)]
fn prop_str(iface: &Value, key: &str) -> Option<String> {
    prop(iface, key)?.as_str().map(str::to_string)
}
#[cfg(test)]
fn prop_bool(iface: &Value, key: &str) -> Option<bool> {
    prop(iface, key)?.as_bool()
}
#[cfg(test)]
fn prop_i64(iface: &Value, key: &str) -> Option<i64> {
    prop(iface, key)?.as_i64()
}

// ---------- device list (D6) ----------

/// Only devices with a real name; paired first, then gamepads (Icon
/// `input-gaming`), then stronger signal first, then by name; at most 20.
pub fn select_devices(mut devices: Vec<BtDeviceInfo>) -> Vec<BtDeviceInfo> {
    devices.retain(|d| d.name.is_some());
    devices.sort_by(|a, b| {
        let key = |d: &BtDeviceInfo| {
            (
                !d.paired,
                d.icon.as_deref() != Some("input-gaming"),
                std::cmp::Reverse(d.rssi.unwrap_or(i16::MIN)),
                d.name.clone().unwrap_or_default().to_lowercase(),
                d.address.clone(),
            )
        };
        key(a).cmp(&key(b))
    });
    devices.truncate(MAX_DEVICES);
    devices
}

/// "gamepad", "audio" or "other" from BlueZ's Icon.
pub fn kind_of(icon: Option<&str>) -> &'static str {
    match icon {
        Some("input-gaming") => "gamepad",
        Some(i) if i.starts_with("audio-") => "audio",
        _ => "other",
    }
}

// ---------- battery fallback ----------

/// Battery percent for `address` from a `/sys/class/power_supply` listing:
/// the entry whose name contains the MAC in lowercase (hid-playstation:
/// `ps-controller-battery-<mac>`); `read_capacity(entry)` returns its
/// `capacity` file parsed.
pub fn battery_from_power_supply(
    address: &str,
    entries: &[String],
    read_capacity: impl Fn(&str) -> Option<u8>,
) -> Option<u8> {
    let mac = address.to_lowercase();
    entries
        .iter()
        .filter(|e| e.to_lowercase().contains(&mac))
        .find_map(|e| read_capacity(e))
        .map(|p| p.min(100))
}

/// Fills in `battery` for devices BlueZ has no Battery1 for.
pub fn fill_battery_fallback(
    devices: &mut [BtDeviceInfo],
    entries: &[String],
    read_capacity: impl Fn(&str) -> Option<u8>,
) {
    for d in devices.iter_mut().filter(|d| d.battery.is_none() && d.connected) {
        d.battery = battery_from_power_supply(&d.address, entries, &read_capacity);
    }
}

// ---------- messages ----------

/// What kind of failure a BlueZ/busctl error is, so callers branch on this
/// and never on the user-facing text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BtErr {
    AccessDenied,
    AuthFailed,
    AlreadyPaired,
    AlreadyConnected,
    InProgress,
    ConnectFailed,
    NotReady,
    Timeout,
    NotFound,
    Unavailable,
    Other,
}

/// Classifies the raw text (which may contain the error name
/// `org.bluez.Error.X` or busctl's rendering of its message, e.g. "Call
/// failed: Authentication Failed"). Matches on letters only.
pub fn classify(raw: &str) -> BtErr {
    let t: String = raw.to_lowercase().chars().filter(|c| c.is_ascii_alphanumeric()).collect();
    let has = |s: &str| t.contains(s);
    if has("accessdenied") || has("permissiondenied") || has("notpermitted") {
        BtErr::AccessDenied
    } else if has("authenticationfailed") || has("authenticationrejected") || has("authenticationcanceled")
        || has("authenticationtimeout")
    {
        BtErr::AuthFailed
    } else if has("alreadyexists") {
        BtErr::AlreadyPaired
    } else if has("alreadyconnected") {
        BtErr::AlreadyConnected
    } else if has("inprogress") {
        BtErr::InProgress
    } else if has("connectionattemptfailed") || has("pagetimeout") || has("hostisdown") {
        BtErr::ConnectFailed
    } else if has("notready") || has("notpowered") {
        BtErr::NotReady
    } else if has("timeout") || has("timedout") || has("noreply") {
        BtErr::Timeout
    } else if has("doesnotexist") || has("doesntexist") || has("unknownobject") || has("unknownmethod") {
        BtErr::NotFound
    } else if has("serviceunknown") || has("namehasnoowner") || has("notprovided") {
        BtErr::Unavailable
    } else {
        BtErr::Other
    }
}

/// A short, human message for a BlueZ/busctl error.
pub fn friendly_error(raw: &str) -> String {
    match classify(raw) {
        BtErr::AccessDenied => "Lumen can't reach Bluetooth (see Settings > Log)",
        BtErr::AuthFailed => "Pairing failed. Put the device in pairing mode and try again",
        BtErr::AlreadyPaired => "Already paired",
        BtErr::AlreadyConnected => "Already connected",
        BtErr::InProgress => "Busy, try again in a moment",
        BtErr::ConnectFailed => "Couldn't connect. Is the device on and in range?",
        BtErr::NotReady => "Bluetooth isn't ready. Turn it on and try again",
        BtErr::Timeout => "Timed out. Try again",
        BtErr::NotFound => "Device not found. Scan again",
        BtErr::Unavailable => "Bluetooth isn't available on this box",
        BtErr::Other => return format!("Bluetooth error: {}", raw.trim().lines().next().unwrap_or("").trim()),
    }
    .to_string()
}

// ---------- UI rows ----------

/// One row of the device list, mirroring Slint's `BtDevice` (main.rs converts).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtRow {
    pub address: String,
    pub name: String,
    pub kind: &'static str,
    pub paired: bool,
    pub connected: bool,
    /// -1 unknown.
    pub battery: i32,
    pub busy: bool,
}

/// Header state, mirroring Slint's `BtState`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BtView {
    pub available: bool,
    pub powered: bool,
    pub scanning: bool,
    pub message: String,
    /// -1 error, 0 normal, 1 good.
    pub level: i32,
}

/// A transient message (pairing result, error) with its level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Note {
    pub text: String,
    pub level: i32,
}

/// Keeps devices already listed (`prev`, in their order) where they are and
/// inserts newcomers by their D6 position. With an empty `prev` this is the
/// D6 order itself.
pub fn stable_order(sorted: Vec<BtDeviceInfo>, prev: &[String]) -> Vec<BtDeviceInfo> {
    let rank = |addr: &str, list: &[BtDeviceInfo]| list.iter().position(|d| d.address == addr);
    let mut out: Vec<BtDeviceInfo> = prev
        .iter()
        .filter_map(|a| rank(a, &sorted).map(|i| sorted[i].clone()))
        .collect();
    for (i, d) in sorted.iter().enumerate() {
        if prev.contains(&d.address) {
            continue;
        }
        // Before the first kept device that D6 would put after it.
        let at = out
            .iter()
            .position(|o| rank(&o.address, &sorted).is_some_and(|r| r > i))
            .unwrap_or(out.len());
        out.insert(at, d.clone());
    }
    out
}

/// The rows to show: D6 filter, `prev` (the addresses listed last time) keeps
/// the order steady; `busy_addr` marks the device being worked on.
pub fn rows(snap: &BtSnapshot, busy_addr: Option<&str>, prev: &[String]) -> Vec<BtRow> {
    stable_order(select_devices(snap.devices.clone()), prev)
        .into_iter()
        .map(|d| BtRow {
            kind: kind_of(d.icon.as_deref()),
            name: d.name.unwrap_or_default(),
            paired: d.paired,
            connected: d.connected,
            battery: d.battery.map_or(-1, i32::from),
            busy: busy_addr == Some(d.address.as_str()),
            address: d.address,
        })
        .collect()
}

/// What the header says: a running action's text (derived from `busy`, so it
/// can't be overwritten) wins over a result note, which wins over a snapshot
/// error.
pub fn shown_note(busy: Option<&str>, note: Option<&Note>, error: Option<&str>) -> Option<Note> {
    busy.map(|t| Note { text: t.to_string(), level: 0 })
        .or_else(|| note.cloned())
        .or_else(|| error.map(|e| Note { text: e.to_string(), level: -1 }))
}

/// Header state. A `note` wins over the scan countdown; `scan_left` is
/// `ScanTimer::remaining_secs`.
pub fn view(snap: &BtSnapshot, scan_left: Option<u32>, note: Option<&Note>) -> BtView {
    let (available, powered) = match &snap.adapter {
        Some(a) => (true, a.powered),
        None => (false, false),
    };
    // "Stop scan" only for a scan Lumen's own timer runs, not for discovery
    // something else started.
    let scanning = powered && scan_left.is_some();
    let (message, level) = match (note, scan_left) {
        (Some(n), _) => (n.text.clone(), n.level),
        (None, Some(s)) if scanning => (format!("Scanning\u{2026} {s} s"), 0),
        _ if !available => ("No Bluetooth adapter found".to_string(), -1),
        _ => (String::new(), 0),
    };
    BtView { available, powered, scanning, message, level }
}

// ---------- scan timer (D7) ----------

/// When the running scan ends. The caller passes the clock, so tests don't
/// sleep.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScanTimer {
    deadline: Option<Instant>,
}

impl ScanTimer {
    pub fn start(&mut self, now: Instant, secs: u32) {
        self.deadline = Some(now + Duration::from_secs(u64::from(secs)));
    }
    pub fn stop(&mut self) {
        self.deadline = None;
    }
    pub fn active(&self) -> bool {
        self.deadline.is_some()
    }
    /// Whole seconds left, rounded up; None when not scanning or expired.
    pub fn remaining_secs(&self, now: Instant) -> Option<u32> {
        let left = self.deadline?.checked_duration_since(now)?;
        if left.is_zero() {
            return None;
        }
        Some(((left.as_millis() + 999) / 1000) as u32)
    }
    /// True once, when the deadline passes; clears the timer.
    pub fn take_expired(&mut self, now: Instant) -> bool {
        match self.deadline {
            Some(d) if now >= d => {
                self.deadline = None;
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FIXTURE: &str = include_str!("testdata/bluez-objects.json");

    fn dev(addr: &str, name: Option<&str>, icon: Option<&str>, paired: bool, rssi: Option<i16>) -> BtDeviceInfo {
        BtDeviceInfo {
            address: addr.into(),
            name: name.map(Into::into),
            icon: icon.map(Into::into),
            paired,
            trusted: paired,
            connected: false,
            rssi,
            battery: None,
        }
    }

    #[test]
    fn parses_fixture() {
        let s = parse_managed_objects(FIXTURE).unwrap();
        let a = s.adapter.unwrap();
        assert_eq!(a.address, "AA:BB:CC:00:00:00");
        assert!(a.powered && a.discovering);
        assert_eq!(s.devices.len(), 4);
        let pad = s.devices.iter().find(|d| d.address == "AA:BB:CC:00:00:01").unwrap();
        assert_eq!(pad.name.as_deref(), Some("DualSense Wireless Controller"));
        assert_eq!(pad.icon.as_deref(), Some("input-gaming"));
        assert!(pad.paired && pad.trusted && pad.connected);
        assert_eq!(pad.rssi, None);
        assert_eq!(pad.battery, None);
        let phone = s.devices.iter().find(|d| d.address == "AA:BB:CC:00:00:02").unwrap();
        assert!(!phone.paired && !phone.connected);
        assert_eq!(phone.rssi, Some(-70));
        let beacon = s.devices.iter().find(|d| d.address == "AA:BB:CC:00:00:03").unwrap();
        assert_eq!(beacon.name, None, "dashed-MAC alias is not a name");
        let spk = s.devices.iter().find(|d| d.address == "AA:BB:CC:00:00:04").unwrap();
        assert_eq!(spk.battery, Some(55), "Battery1 on the device path");
    }

    #[test]
    fn bluez_paths_keep_adapter_and_devices_only() {
        let list = "/org/bluez\n/org/bluez/hci0\n/org/bluez/hci0/dev_AA_BB_CC_00_00_01\n\
                    /org/bluez/hci0/dev_AA_BB_CC_00_00_01/fd0\n/org/bluez/hci0/dev_AA_BB_CC_00_00_0\n\
                    /org/bluez/hci0/dev_AA_BB_CC_00_00_02\n/org/bluez/agent\n";
        let (a, d) = parse_bluez_paths(list);
        assert_eq!(a.as_deref(), Some("/org/bluez/hci0"));
        assert_eq!(d, ["/org/bluez/hci0/dev_AA_BB_CC_00_00_01", "/org/bluez/hci0/dev_AA_BB_CC_00_00_02"]);
        assert_eq!(parse_bluez_paths("").0, None);
        let many: String = (0..60).map(|i| format!("/org/bluez/hci0/dev_AA_BB_CC_00_00_{i:02X}\n")).collect();
        assert_eq!(parse_bluez_paths(&many).1.len(), MAX_DEVICE_PATHS);
    }

    #[test]
    fn one_unparseable_property_does_not_sink_the_device() {
        // Third line is what a failed JSON writer leaves behind.
        let out = "{\"type\":\"s\",\"data\":\"AA:BB:CC:00:00:05\"}\n\
                   {\"type\":\"s\",\"data\":\"Pad 5\"}\n\
                   Failed to create new json object: Invalid argument\n\
                   {\"type\":\"b\",\"data\":true}\n\
                   {\"type\":\"b\",\"data\":false}\n";
        let vals = parse_prop_lines(out);
        assert_eq!(vals.len(), 5);
        assert!(vals[2].is_none());
        let d = device_from_props(&["Address", "Alias", "Icon", "Paired", "Connected"], &vals, Some(250)).unwrap();
        assert_eq!(d.address, "AA:BB:CC:00:00:05");
        assert_eq!(d.name.as_deref(), Some("Pad 5"));
        assert_eq!(d.icon, None);
        assert!(d.paired && !d.connected);
        assert_eq!(d.battery, Some(100));
        // No Address at all: skipped, not an error.
        assert!(device_from_props(&["Address"], &[None], None).is_none());
    }

    #[test]
    fn device_props_mac_alias_is_not_a_name_and_rssi_parses() {
        let vals = parse_prop_lines(
            "{\"type\":\"s\",\"data\":\"AA:BB:CC:00:00:06\"}\n{\"type\":\"s\",\"data\":\"AA-BB-CC-00-00-06\"}\n{\"type\":\"n\",\"data\":-70}\n",
        );
        let d = device_from_props(&["Address", "Alias", "RSSI"], &vals, None).unwrap();
        assert_eq!(d.name, None);
        assert_eq!(d.rssi, Some(-70));
        let vals = parse_prop_lines("{\"type\":\"s\",\"data\":\"AA:BB:CC:00:00:00\"}\n{\"type\":\"b\",\"data\":true}\n");
        let a = adapter_from_props(&["Address", "Powered", "Discovering"], &vals);
        assert!(a.powered && !a.discovering);
    }

    #[test]
    fn parse_rejects_garbage_and_handles_no_adapter() {
        assert!(parse_managed_objects("nope").is_err());
        assert!(parse_managed_objects("{\"type\":\"x\",\"data\":[]}").is_err());
        let s = parse_managed_objects("{\"type\":\"a{oa{sa{sv}}}\",\"data\":[{}]}").unwrap();
        assert!(s.adapter.is_none() && s.devices.is_empty());
    }

    #[test]
    fn select_filters_and_orders() {
        let s = parse_managed_objects(FIXTURE).unwrap();
        let names: Vec<_> = select_devices(s.devices).into_iter().map(|d| d.name.unwrap()).collect();
        // Unnamed beacon dropped; paired (gamepad first), then the phone.
        assert_eq!(names, ["DualSense Wireless Controller", "Speaker 1", "Phone 1"]);
    }

    #[test]
    fn select_order_unpaired_gamepad_then_signal() {
        let v = vec![
            dev("A", Some("Weak"), None, false, Some(-90)),
            dev("B", Some("Strong"), None, false, Some(-40)),
            dev("C", Some("Pad"), Some("input-gaming"), false, Some(-80)),
            dev("D", Some("NoSignal"), None, false, None),
            dev("E", Some("Paired"), None, true, None),
        ];
        let names: Vec<_> = select_devices(v).into_iter().map(|d| d.name.unwrap()).collect();
        assert_eq!(names, ["Paired", "Pad", "Strong", "Weak", "NoSignal"]);
    }

    #[test]
    fn select_caps_at_20() {
        let v: Vec<_> = (0..30).map(|i| dev(&format!("A{i}"), Some(&format!("D{i:02}")), None, false, Some(-50))).collect();
        assert_eq!(select_devices(v).len(), MAX_DEVICES);
    }

    #[test]
    fn kinds() {
        assert_eq!(kind_of(Some("input-gaming")), "gamepad");
        assert_eq!(kind_of(Some("audio-card")), "audio");
        assert_eq!(kind_of(Some("audio-headset")), "audio");
        assert_eq!(kind_of(Some("phone")), "other");
        assert_eq!(kind_of(None), "other");
    }

    #[test]
    fn battery_fallback() {
        let entries: Vec<String> = ["BAT0", "ps-controller-battery-aa:bb:cc:00:00:01", "ps-controller-battery-aa:bb:cc:00:00:09"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let read = |e: &str| match e {
            "ps-controller-battery-aa:bb:cc:00:00:01" => Some(70),
            "ps-controller-battery-aa:bb:cc:00:00:09" => Some(20),
            _ => None,
        };
        assert_eq!(battery_from_power_supply("AA:BB:CC:00:00:01", &entries, read), Some(70));
        assert_eq!(battery_from_power_supply("AA:BB:CC:00:00:02", &entries, read), None);
        assert_eq!(battery_from_power_supply("AA:BB:CC:00:00:01", &entries, |_| None), None);
        assert_eq!(battery_from_power_supply("AA:BB:CC:00:00:01", &entries, |_| Some(250)), Some(100));

        let mut devs = vec![dev("AA:BB:CC:00:00:01", Some("Pad"), None, true, None)];
        devs[0].connected = true;
        let mut off = dev("AA:BB:CC:00:00:09", Some("Off"), None, true, None);
        off.battery = None;
        devs.push(off);
        fill_battery_fallback(&mut devs, &entries, read);
        assert_eq!(devs[0].battery, Some(70));
        assert_eq!(devs[1].battery, None, "disconnected devices are skipped");
    }

    #[test]
    fn friendly_messages() {
        assert!(friendly_error("Call failed: Authentication Failed").starts_with("Pairing failed"));
        assert!(friendly_error("org.bluez.Error.AuthenticationFailed").starts_with("Pairing failed"));
        assert_eq!(friendly_error("Call failed: Already Exists"), "Already paired");
        assert_eq!(friendly_error("org.bluez.Error.InProgress"), "Busy, try again in a moment");
        assert!(friendly_error("org.bluez.Error.ConnectionAttemptFailed").starts_with("Couldn't connect"));
        assert!(friendly_error("Call failed: Operation already in progress").starts_with("Busy"));
        assert!(friendly_error("org.bluez.Error.NotReady").starts_with("Bluetooth isn't ready"));
        assert!(friendly_error("Method call timed out").starts_with("Timed out"));
        assert!(friendly_error("Access denied").contains("can't reach Bluetooth"));
        // Seen on the Pi when the device object is gone.
        assert_eq!(
            friendly_error("Call failed: Method \"Connect\" with signature \"\" on interface \"org.bluez.Device1\" doesn't exist"),
            "Device not found. Scan again"
        );
        assert_eq!(friendly_error("Call failed: Already Connected"), "Already connected");
        assert!(friendly_error("Call failed: something odd").starts_with("Bluetooth error"));
        assert_eq!(friendly_error("weird\nsecond line"), "Bluetooth error: weird");
    }

    #[test]
    fn rows_and_view() {
        let s = parse_managed_objects(FIXTURE).unwrap();
        let r = rows(&s, Some("AA:BB:CC:00:00:02"), &[]);
        assert_eq!(r.len(), 3);
        assert_eq!((r[0].kind, r[0].connected, r[0].battery, r[0].busy), ("gamepad", true, -1, false));
        assert_eq!((r[1].kind, r[1].battery), ("audio", 55));
        assert!(r[2].busy && !r[2].paired);

        let v = view(&s, Some(42), None);
        assert!(v.available && v.powered && v.scanning);
        assert_eq!(v.message, "Scanning\u{2026} 42 s");
        let note = Note { text: "Pairing DualSense\u{2026}".into(), level: 0 };
        assert_eq!(view(&s, Some(42), Some(&note)).message, note.text);
        let none = view(&BtSnapshot::default(), None, None);
        assert!(!none.available);
        assert_eq!(none.level, -1);
        let mut off = s.clone();
        off.adapter.as_mut().unwrap().powered = false;
        assert!(!view(&off, Some(5), None).scanning);
    }

    #[test]
    fn scan_timer() {
        let t0 = Instant::now();
        let mut t = ScanTimer::default();
        assert!(!t.active() && t.remaining_secs(t0).is_none());
        t.start(t0, SCAN_SECS);
        assert_eq!(t.remaining_secs(t0), Some(60));
        assert_eq!(t.remaining_secs(t0 + Duration::from_millis(1500)), Some(59));
        assert!(!t.take_expired(t0 + Duration::from_secs(59)));
        assert!(t.take_expired(t0 + Duration::from_secs(60)));
        assert!(!t.active());
        assert!(!t.take_expired(t0 + Duration::from_secs(61)), "only once");
        t.start(t0, 10);
        t.stop();
        assert!(t.remaining_secs(t0).is_none());
    }

    #[test]
    fn friendly_error_covers_every_branch() {
        let cases = [
            ("org.freedesktop.DBus.Error.AccessDenied", "Lumen can't reach Bluetooth (see Settings > Log)"),
            ("Permission denied", "Lumen can't reach Bluetooth (see Settings > Log)"),
            ("org.bluez.Error.NotPermitted", "Lumen can't reach Bluetooth (see Settings > Log)"),
            ("org.bluez.Error.AuthenticationRejected", "Pairing failed. Put the device in pairing mode and try again"),
            ("org.bluez.Error.AuthenticationCanceled", "Pairing failed. Put the device in pairing mode and try again"),
            ("org.bluez.Error.AuthenticationTimeout", "Pairing failed. Put the device in pairing mode and try again"),
            ("org.bluez.Error.AlreadyExists", "Already paired"),
            ("org.bluez.Error.AlreadyConnected", "Already connected"),
            ("Call failed: Page Timeout", "Couldn't connect. Is the device on and in range?"),
            ("Host is down", "Couldn't connect. Is the device on and in range?"),
            ("org.bluez.Error.NotPowered", "Bluetooth isn't ready. Turn it on and try again"),
            ("Connection timed out", "Timed out. Try again"),
            ("Did not receive a reply (NoReply)", "Timed out. Try again"),
            ("org.bluez.Error.DoesNotExist", "Device not found. Scan again"),
            ("org.freedesktop.DBus.Error.UnknownObject", "Device not found. Scan again"),
            ("org.freedesktop.DBus.Error.UnknownMethod", "Device not found. Scan again"),
            ("org.freedesktop.DBus.Error.ServiceUnknown", "Bluetooth isn't available on this box"),
            ("org.freedesktop.DBus.Error.NameHasNoOwner", "Bluetooth isn't available on this box"),
            ("The name org.bluez was not provided by any .service files", "Bluetooth isn't available on this box"),
        ];
        for (raw, want) in cases {
            assert_eq!(friendly_error(raw), want, "{raw}");
        }
    }

    #[test]
    fn friendly_error_unknown_and_empty_text() {
        assert_eq!(friendly_error(""), "Bluetooth error: ");
        assert_eq!(friendly_error("  \n  padded first line  \nsecond"), "Bluetooth error: padded first line");
        assert_eq!(friendly_error("  padded  \nsecond"), "Bluetooth error: padded");
    }

    #[test]
    fn select_ties_break_by_name_case_insensitively_then_address() {
        let v = vec![
            dev("C", Some("beta"), None, false, Some(-50)),
            dev("B", Some("Alpha"), None, false, Some(-50)),
            dev("A", Some("alpha"), None, false, Some(-50)),
        ];
        let addrs: Vec<_> = select_devices(v).into_iter().map(|d| d.address).collect();
        assert_eq!(addrs, ["A", "B", "C"], "alpha/Alpha tie falls to address");
    }

    #[test]
    fn select_cap_keeps_the_best_and_ignores_unnamed_for_the_count() {
        let mut v: Vec<_> = (0..25).map(|i| dev(&format!("A{i}"), Some(&format!("D{i:02}")), None, false, Some(-90 + i as i16))).collect();
        v.extend((0..10).map(|i| dev(&format!("U{i}"), None, None, true, Some(-10))));
        let out = select_devices(v);
        assert_eq!(out.len(), MAX_DEVICES);
        assert_eq!(out[0].address, "A24", "strongest signal first");
        assert!(out.iter().all(|d| d.address.starts_with('A')), "unnamed dropped before the cap");
        assert!(out.iter().all(|d| d.address != "A0"), "weakest are cut");
    }

    #[test]
    fn select_empty_input() {
        assert!(select_devices(Vec::new()).is_empty());
    }

    #[test]
    fn scan_timer_edges() {
        let t0 = Instant::now();
        let mut t = ScanTimer::default();
        assert!(!t.take_expired(t0), "never started");
        t.start(t0, 10);
        assert_eq!(t.remaining_secs(t0 + Duration::from_millis(9_999)), Some(1), "rounds up");
        assert_eq!(t.remaining_secs(t0 + Duration::from_secs(10)), None, "none at the deadline");
        assert!(t.active(), "still active until take_expired runs");
        // Restarting replaces the deadline.
        t.start(t0 + Duration::from_secs(5), 10);
        assert!(!t.take_expired(t0 + Duration::from_secs(14)));
        assert!(t.take_expired(t0 + Duration::from_secs(15)));
        // Zero seconds expires immediately.
        t.start(t0, 0);
        assert_eq!(t.remaining_secs(t0), None);
        assert!(t.take_expired(t0));
        // A clock before the start doesn't panic.
        t.start(t0 + Duration::from_secs(5), 3);
        assert_eq!(t.remaining_secs(t0), Some(8));
    }

    #[test]
    fn view_edge_states() {
        let s = parse_managed_objects(FIXTURE).unwrap();
        // BlueZ discovering without Lumen's timer (someone else's scan): not
        // ours to stop, so no "Stop scan" and no countdown text.
        let v = view(&s, None, None);
        assert!(!v.scanning);
        assert_eq!((v.message.as_str(), v.level), ("", 0));
        // No adapter: a note still wins over the "no adapter" text.
        let note = Note { text: "oops".into(), level: -1 };
        let v = view(&BtSnapshot::default(), None, Some(&note));
        assert!(!v.available && !v.scanning);
        assert_eq!((v.message.as_str(), v.level), ("oops", -1));
        // Adapter off with a leftover timer: no scanning text, no "no adapter" text.
        let mut off = s.clone();
        off.adapter.as_mut().unwrap().powered = false;
        off.adapter.as_mut().unwrap().discovering = false;
        let v = view(&off, Some(5), None);
        assert_eq!((v.available, v.powered, v.scanning, v.message.as_str()), (true, false, false, ""));
    }

    #[test]
    fn battery_fallback_prefers_existing_and_skips_unreadable() {
        let entries = vec!["ps-controller-battery-aa:bb:cc:00:00:01".to_string()];
        let mut d = dev("AA:BB:CC:00:00:01", Some("Pad"), None, true, None);
        d.connected = true;
        d.battery = Some(33);
        let mut devs = vec![d.clone()];
        fill_battery_fallback(&mut devs, &entries, |_| Some(90));
        assert_eq!(devs[0].battery, Some(33), "BlueZ's Battery1 is kept");
        devs[0].battery = None;
        fill_battery_fallback(&mut devs, &entries, |_| None);
        assert_eq!(devs[0].battery, None, "unreadable capacity stays unknown");
        assert_eq!(battery_from_power_supply("AA:BB:CC:00:00:01", &[], |_| Some(1)), None);
    }

    #[test]
    fn classify_names_the_kind_and_friendly_error_follows() {
        assert_eq!(classify("org.bluez.Error.AlreadyExists"), BtErr::AlreadyPaired);
        assert_eq!(classify("Call failed: Already Exists"), BtErr::AlreadyPaired);
        assert_eq!(classify("Call failed: Already Connected"), BtErr::AlreadyConnected);
        assert_eq!(classify("Call failed: Authentication Failed"), BtErr::AuthFailed);
        assert_eq!(classify("org.bluez.Error.ConnectionAttemptFailed"), BtErr::ConnectFailed);
        assert_eq!(classify("something odd"), BtErr::Other);
        assert_eq!(classify(""), BtErr::Other);
    }

    fn addrs(v: &[BtDeviceInfo]) -> Vec<&str> {
        v.iter().map(|d| d.address.as_str()).collect()
    }

    #[test]
    fn stable_order_first_list_is_d6() {
        let sorted = select_devices(vec![
            dev("B", Some("Weak"), None, false, Some(-90)),
            dev("A", Some("Strong"), None, false, Some(-40)),
        ]);
        assert_eq!(addrs(&stable_order(sorted, &[])), ["A", "B"]);
    }

    #[test]
    fn stable_order_keeps_listed_devices_in_place_when_signal_swaps() {
        let prev: Vec<String> = vec!["A".into(), "B".into(), "C".into()];
        // C is now the strongest, but the rows don't move under the focus.
        let now = select_devices(vec![
            dev("A", Some("A"), None, false, Some(-80)),
            dev("B", Some("B"), None, false, Some(-70)),
            dev("C", Some("C"), None, false, Some(-30)),
        ]);
        assert_eq!(addrs(&now), ["C", "B", "A"]);
        assert_eq!(addrs(&stable_order(now, &prev)), ["A", "B", "C"]);
    }

    #[test]
    fn stable_order_places_newcomers_by_d6_and_drops_vanished() {
        let prev: Vec<String> = vec!["A".into(), "B".into(), "GONE".into()];
        let now = select_devices(vec![
            dev("A", Some("A"), None, false, Some(-40)),
            dev("B", Some("B"), None, false, Some(-80)),
            dev("N1", Some("N1"), None, false, Some(-60)), // between A and B
            dev("N2", Some("N2"), None, false, Some(-95)), // last
            dev("P", Some("P"), None, true, None),         // paired: D6 puts it first
        ]);
        assert_eq!(addrs(&stable_order(now, &prev)), ["P", "A", "N1", "B", "N2"]);
    }

    #[test]
    fn rows_use_the_previous_order() {
        let s = parse_managed_objects(FIXTURE).unwrap();
        let first = rows(&s, None, &[]);
        let mut prev: Vec<String> = first.iter().map(|r| r.address.clone()).collect();
        prev.reverse();
        let again = rows(&s, None, &prev);
        let got: Vec<_> = again.iter().map(|r| r.address.clone()).collect();
        assert_eq!(got, prev);
    }

    #[test]
    fn shown_note_priority_is_busy_then_note_then_error() {
        let n = Note { text: "Paired".into(), level: 1 };
        let b = shown_note(Some("Pairing\u{2026}"), Some(&n), Some("err")).unwrap();
        assert_eq!((b.text.as_str(), b.level), ("Pairing\u{2026}", 0));
        let m = shown_note(None, Some(&n), Some("err")).unwrap();
        assert_eq!(m.text, "Paired");
        let e = shown_note(None, None, Some("err")).unwrap();
        assert_eq!((e.text.as_str(), e.level), ("err", -1));
        assert!(shown_note(None, None, None).is_none());
    }
}
