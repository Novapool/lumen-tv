// Deterministic Bluetooth for Mac development and for the TV demo
// (`LUMEN_BT=fake`). One powered adapter. After a scan starts, three devices
// show up: "DualSense Wireless Controller" at 2 s, "Phone 1" at 3 s and
// "Fail Pad" at 4 s. Pair takes 1.5 s and ends connected (the DualSense with
// battery 70); "Fail Pad" fails with AuthenticationFailed.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use super::{Bluetooth, BtAdapter, BtDeviceInfo, BtSnapshot};

const ADAPTER: &str = "AA:BB:CC:00:00:00";

struct FakeDev {
    info: BtDeviceInfo,
    appears_ms: u64,
    present: bool,
    /// Forgotten during the current scan: stays away until the next scan.
    removed: bool,
    fails: bool,
}

struct State {
    powered: bool,
    scan: Option<(Instant, u32)>,
    devices: Vec<FakeDev>,
}

pub struct FakeBt {
    state: Mutex<State>,
    pair_delay: Duration,
}

impl Default for FakeBt {
    fn default() -> Self {
        Self::new(Duration::from_millis(1500))
    }
}

impl FakeBt {
    fn new(pair_delay: Duration) -> Self {
        let dev = |n: u8, name: &str, icon: &str, rssi: i16, appears_ms: u64, fails: bool| FakeDev {
            info: BtDeviceInfo {
                address: format!("AA:BB:CC:00:00:0{n}"),
                name: Some(name.to_string()),
                icon: Some(icon.to_string()),
                paired: false,
                trusted: false,
                connected: false,
                rssi: Some(rssi),
                battery: None,
            },
            appears_ms,
            present: false,
            removed: false,
            fails,
        };
        FakeBt {
            state: Mutex::new(State {
                powered: true,
                scan: None,
                devices: vec![
                    dev(1, "DualSense Wireless Controller", "input-gaming", -45, 2000, false),
                    dev(2, "Phone 1", "phone", -70, 3000, false),
                    dev(3, "Fail Pad", "input-gaming", -60, 4000, true),
                ],
            }),
            pair_delay,
        }
    }

    fn scanning(s: &State, now: Instant) -> bool {
        s.powered && s.scan.is_some_and(|(t, secs)| now.duration_since(t) < Duration::from_secs(u64::from(secs)))
    }

    fn snapshot_at(&self, now: Instant) -> BtSnapshot {
        let mut s = self.state.lock().unwrap();
        if Self::scanning(&s, now) {
            let ms = s.scan.map_or(0, |(t, _)| now.duration_since(t).as_millis() as u64);
            for d in s.devices.iter_mut().filter(|d| !d.removed && ms >= d.appears_ms) {
                d.present = true;
            }
        }
        BtSnapshot {
            adapter: Some(BtAdapter {
                address: ADAPTER.to_string(),
                powered: s.powered,
                discovering: Self::scanning(&s, now),
            }),
            devices: s.devices.iter().filter(|d| d.present).map(|d| d.info.clone()).collect(),
            scan_error: None,
        }
    }

    fn start_scan_at(&self, now: Instant, secs: u32) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        if !s.powered {
            return Err("org.bluez.Error.NotReady".into());
        }
        s.scan = Some((now, secs));
        for d in &mut s.devices {
            d.removed = false;
        }
        Ok(())
    }

    fn with_dev<T>(&self, addr: &str, f: impl FnOnce(&mut FakeDev) -> T) -> Result<T, String> {
        let mut s = self.state.lock().unwrap();
        match s.devices.iter_mut().find(|d| d.present && d.info.address == addr) {
            Some(d) => Ok(f(d)),
            None => Err("org.bluez.Error.DoesNotExist".into()),
        }
    }
}

impl Bluetooth for FakeBt {
    fn snapshot(&self) -> Result<BtSnapshot, String> {
        Ok(self.snapshot_at(Instant::now()))
    }

    fn set_powered(&self, on: bool) -> Result<(), String> {
        let mut s = self.state.lock().unwrap();
        s.powered = on;
        if !on {
            s.scan = None;
            for d in &mut s.devices {
                d.info.connected = false;
                d.info.battery = None;
            }
        }
        Ok(())
    }

    fn start_scan(&self, secs: u32) -> Result<(), String> {
        self.start_scan_at(Instant::now(), secs)
    }

    fn stop_scan(&self) -> Result<(), String> {
        self.state.lock().unwrap().scan = None;
        Ok(())
    }

    fn pair(&self, addr: &str) -> Result<(), String> {
        let fails = self.with_dev(addr, |d| d.fails)?;
        std::thread::sleep(self.pair_delay);
        if fails {
            return Err("Call failed: Authentication Failed".into());
        }
        self.with_dev(addr, |d| {
            d.info.paired = true;
            d.info.trusted = true;
            d.info.connected = true;
            d.info.battery = (d.info.icon.as_deref() == Some("input-gaming")).then_some(70);
        })
    }

    fn connect(&self, addr: &str) -> Result<(), String> {
        self.with_dev(addr, |d| {
            d.info.connected = true;
            d.info.battery = (d.info.icon.as_deref() == Some("input-gaming")).then_some(70);
        })
    }

    fn disconnect(&self, addr: &str) -> Result<(), String> {
        self.with_dev(addr, |d| {
            d.info.connected = false;
            d.info.battery = None;
        })
    }

    fn forget(&self, addr: &str) -> Result<(), String> {
        self.with_dev(addr, |d| {
            d.present = false;
            d.removed = true;
            d.info.paired = false;
            d.info.trusted = false;
            d.info.connected = false;
            d.info.battery = None;
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAD: &str = "AA:BB:CC:00:00:01";
    const PHONE: &str = "AA:BB:CC:00:00:02";
    const FAIL: &str = "AA:BB:CC:00:00:03";

    fn at(t0: Instant, ms: u64) -> Instant {
        t0 + Duration::from_millis(ms)
    }

    #[test]
    fn devices_appear_on_schedule() {
        let bt = FakeBt::new(Duration::ZERO);
        let t0 = Instant::now();
        assert!(bt.snapshot_at(t0).devices.is_empty(), "nothing before a scan");
        bt.start_scan_at(t0, 60).unwrap();
        let s = bt.snapshot_at(at(t0, 1999));
        assert!(s.devices.is_empty() && s.adapter.as_ref().unwrap().discovering);
        assert_eq!(bt.snapshot_at(at(t0, 2000)).devices.len(), 1);
        assert_eq!(bt.snapshot_at(at(t0, 3500)).devices.len(), 2);
        let s = bt.snapshot_at(at(t0, 4000));
        assert_eq!(s.devices.len(), 3);
        assert_eq!(s.devices[0].name.as_deref(), Some("DualSense Wireless Controller"));
        // Scan end: devices stay, discovering goes false.
        let s = bt.snapshot_at(at(t0, 61_000));
        assert_eq!(s.devices.len(), 3);
        assert!(!s.adapter.unwrap().discovering);
    }

    #[test]
    fn stop_scan_freezes_the_list() {
        let bt = FakeBt::new(Duration::ZERO);
        let t0 = Instant::now();
        bt.start_scan_at(t0, 60).unwrap();
        bt.snapshot_at(at(t0, 2500));
        bt.stop_scan().unwrap();
        assert_eq!(bt.snapshot_at(at(t0, 5000)).devices.len(), 1);
    }

    fn scanned() -> (FakeBt, Instant) {
        let bt = FakeBt::new(Duration::ZERO);
        let t0 = Instant::now();
        bt.start_scan_at(t0, 60).unwrap();
        bt.snapshot_at(at(t0, 4000));
        (bt, t0)
    }

    #[test]
    fn pair_connects_with_battery() {
        let (bt, t0) = scanned();
        bt.pair(PAD).unwrap();
        bt.pair(PHONE).unwrap();
        let s = bt.snapshot_at(at(t0, 4001));
        let pad = s.devices.iter().find(|d| d.address == PAD).unwrap();
        assert!(pad.paired && pad.trusted && pad.connected);
        assert_eq!(pad.battery, Some(70));
        let phone = s.devices.iter().find(|d| d.address == PHONE).unwrap();
        assert!(phone.paired && phone.connected);
        assert_eq!(phone.battery, None);
    }

    #[test]
    fn fail_pad_fails_and_stays_unpaired() {
        let (bt, t0) = scanned();
        let e = bt.pair(FAIL).unwrap_err();
        assert!(crate::bluetooth::friendly_error(&e).starts_with("Pairing failed"));
        let s = bt.snapshot_at(at(t0, 4001));
        assert!(!s.devices.iter().find(|d| d.address == FAIL).unwrap().paired);
    }

    #[test]
    fn disconnect_connect_forget() {
        let (bt, t0) = scanned();
        bt.pair(PAD).unwrap();
        bt.disconnect(PAD).unwrap();
        let d = bt.snapshot_at(at(t0, 4001)).devices.into_iter().find(|d| d.address == PAD).unwrap();
        assert!(d.paired && !d.connected && d.battery.is_none());
        bt.connect(PAD).unwrap();
        bt.forget(PAD).unwrap();
        let s = bt.snapshot_at(at(t0, 4002));
        assert!(s.devices.iter().all(|d| d.address != PAD), "forgotten device is gone");
        assert!(bt.connect(PAD).is_err());
        // Back on the next scan, unpaired.
        bt.start_scan_at(at(t0, 5000), 60).unwrap();
        let d = bt.snapshot_at(at(t0, 7000)).devices.into_iter().find(|d| d.address == PAD).unwrap();
        assert!(!d.paired);
    }

    #[test]
    fn power_off_blocks_scan() {
        let bt = FakeBt::new(Duration::ZERO);
        bt.set_powered(false).unwrap();
        assert!(!bt.snapshot().unwrap().adapter.unwrap().powered);
        assert!(bt.start_scan(10).is_err());
        bt.set_powered(true).unwrap();
        assert!(bt.start_scan(10).is_ok());
    }

    #[test]
    fn actions_on_unseen_devices_fail_as_not_found() {
        let bt = FakeBt::new(Duration::ZERO);
        // Before any scan nothing is present.
        for e in [
            bt.pair(PAD).unwrap_err(),
            bt.connect(PAD).unwrap_err(),
            bt.disconnect(PAD).unwrap_err(),
            bt.forget(PAD).unwrap_err(),
            bt.pair("AA:BB:CC:00:00:99").unwrap_err(),
        ] {
            assert_eq!(crate::bluetooth::friendly_error(&e), "Device not found. Scan again");
        }
    }

    #[test]
    fn short_scan_never_reveals_later_devices() {
        let bt = FakeBt::new(Duration::ZERO);
        let t0 = Instant::now();
        bt.start_scan_at(t0, 3).unwrap();
        // The 4 s device would show at 4000 ms, but the 3 s scan is over.
        let s = bt.snapshot_at(at(t0, 5000));
        let names: Vec<_> = s.devices.iter().map(|d| d.address.as_str()).collect();
        assert!(!names.contains(&FAIL));
    }

    #[test]
    fn power_off_disconnects_and_clears_battery_but_keeps_pairing() {
        let (bt, t0) = scanned();
        bt.pair(PAD).unwrap();
        bt.set_powered(false).unwrap();
        let s = bt.snapshot_at(at(t0, 4001));
        assert!(!s.adapter.as_ref().unwrap().discovering);
        let pad = s.devices.iter().find(|d| d.address == PAD).unwrap();
        assert!(pad.paired && !pad.connected && pad.battery.is_none());
    }

    #[test]
    fn forgotten_device_stays_away_for_the_rest_of_the_scan_only() {
        let (bt, t0) = scanned();
        bt.forget(PHONE).unwrap();
        assert!(bt.snapshot_at(at(t0, 30_000)).devices.iter().all(|d| d.address != PHONE));
        bt.start_scan_at(at(t0, 31_000), 60).unwrap();
        assert!(bt.snapshot_at(at(t0, 34_000)).devices.iter().any(|d| d.address == PHONE));
    }

    #[test]
    fn failed_pair_can_be_retried_and_fails_the_same_way() {
        let (bt, _) = scanned();
        assert!(bt.pair(FAIL).is_err());
        assert!(bt.pair(FAIL).is_err());
        // The fake pad is still there to forget.
        assert!(bt.forget(FAIL).is_ok());
    }

    #[test]
    fn connect_gives_a_gamepad_battery_and_a_phone_none() {
        let (bt, t0) = scanned();
        bt.connect(PAD).unwrap();
        bt.connect(PHONE).unwrap();
        let s = bt.snapshot_at(at(t0, 4001));
        let get = |a: &str| s.devices.iter().find(|d| d.address == a).unwrap().clone();
        assert_eq!((get(PAD).connected, get(PAD).battery), (true, Some(70)));
        assert_eq!((get(PHONE).connected, get(PHONE).battery), (true, None));
        assert!(!get(PAD).paired, "connect alone doesn't pair");
    }
}
