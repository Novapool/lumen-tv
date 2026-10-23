// Mapping files (config/input/*.toml, overridable per file name in
// ~/.config/lumen-tv/input/): which file a device uses, and how its raw evdev
// events become action presses and releases. Codes are plain numbers here, so
// this is pure logic; devices.rs resolves the code names in the files.

use std::collections::HashMap;
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::Action;

pub const EV_KEY: u16 = 1;
pub const EV_ABS: u16 = 3;
// Kernel codes used to tell device classes apart (linux/input-event-codes.h).
const KEY_ENTER: u16 = 28;
const KEY_A: u16 = 30;
const BTN_SOUTH: u16 = 0x130;

const DEFAULT_DEADZONE: f32 = 0.4;
/// A stick that has crossed the deadzone stays pressed until it falls back
/// below this fraction of it, so a stick resting near the edge can't flicker.
const RELEASE_FRACTION: f32 = 0.75;
/// After an analog stick releases a direction, a press in the opposite
/// direction on the same axis is ignored for this long. A thumb flick springs
/// back past centre and briefly overshoots into the other side; without this
/// the overshoot fires the opposite action and focus snaps back. A deliberate
/// reversal takes longer than this. If the opposite deflection is still held
/// when the window ends, `DeviceMap::tick` fires it (a stick held steady at
/// full deflection sends no further events).
const REBOUND_LOCKOUT: Duration = Duration::from_millis(150);

/// The mapping files built into Lumen.
pub const SHIPPED: [(&str, &str); 5] = [
    ("keyboard.toml", include_str!("../../config/input/keyboard.toml")),
    ("cec-remote.toml", include_str!("../../config/input/cec-remote.toml")),
    ("gamepad.toml", include_str!("../../config/input/gamepad.toml")),
    ("xbox.toml", include_str!("../../config/input/xbox.toml")),
    ("playstation.toml", include_str!("../../config/input/playstation.toml")),
];

/// What a device reports about itself when it is opened.
pub struct DeviceInfo {
    pub name: String,
    pub vendor: u16,
    pub product: u16,
    pub keys: Vec<u16>,
    /// (code, minimum, maximum) of each absolute axis.
    pub axes: Vec<(u16, i32, i32)>,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
enum Class {
    Keyboard,
    Gamepad,
}

impl Class {
    fn of(dev: &DeviceInfo) -> Option<Class> {
        if dev.keys.contains(&BTN_SOUTH) {
            Some(Class::Gamepad)
        } else if dev.keys.contains(&KEY_A) && dev.keys.contains(&KEY_ENTER) {
            Some(Class::Keyboard)
        } else {
            None
        }
    }
}

/// A gamepad is a device with BTN_SOUTH (the same test the mapping files use).
pub fn is_gamepad(dev: &DeviceInfo) -> bool {
    Class::of(dev) == Some(Class::Gamepad)
}

#[derive(Deserialize, Clone)]
#[serde(deny_unknown_fields)]
struct MatchRules {
    /// If set, the device must be of this class.
    class: Option<Class>,
    /// "045e" (any product from vendor 045e) or "045e:028e", hex.
    #[serde(default)]
    ids: Vec<String>,
    /// Case-insensitive substrings of the device name.
    #[serde(default)]
    names: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MappingFile {
    deadzone: Option<f32>,
    #[serde(rename = "match")]
    rules: MatchRules,
    #[serde(default)]
    keys: HashMap<String, Action>,
    #[serde(default)]
    axes: HashMap<String, [Action; 2]>,
}

/// One parsed mapping file, with code names resolved.
#[derive(Clone)]
pub struct Mapping {
    pub file: String,
    rules: MatchRules,
    deadzone: f32,
    keys: HashMap<u16, Action>,
    /// code → [action below centre, action above centre]
    axes: HashMap<u16, [Action; 2]>,
}

impl Mapping {
    /// `resolve` turns a code name like "KEY_UP" or "ABS_X" into (event type, code).
    pub fn parse(file: &str, text: &str, resolve: &impl Fn(&str) -> Option<(u16, u16)>) -> Result<Mapping, String> {
        let f: MappingFile = toml::from_str(text).map_err(|e| format!("{file}: {e}"))?;
        let code = |name: &str, kind: u16, table: &str| match resolve(name) {
            Some((k, code)) if k == kind => Ok(code),
            _ => Err(format!("{file}: [{table}] {name} is not a known {table} code")),
        };
        let keys = f.keys.iter().map(|(name, &a)| Ok((code(name, EV_KEY, "keys")?, a))).collect::<Result<_, String>>()?;
        let axes = f.axes.iter().map(|(name, &a)| Ok((code(name, EV_ABS, "axes")?, a))).collect::<Result<_, String>>()?;
        if let Some(id) = f.rules.ids.iter().find(|id| parse_id(id).is_none()) {
            return Err(format!("{file}: id {id:?} should look like \"045e\" or \"045e:028e\""));
        }
        let deadzone = f.deadzone.unwrap_or(DEFAULT_DEADZONE);
        if !(0.05..=0.95).contains(&deadzone) {
            return Err(format!("{file}: deadzone must be between 0.05 and 0.95"));
        }
        if f.rules.class.is_none() && f.rules.ids.is_empty() && f.rules.names.is_empty() {
            return Err(format!("{file}: [match] needs a class, ids or names"));
        }
        Ok(Mapping { file: file.to_string(), rules: f.rules, deadzone, keys, axes })
    }

    /// How specifically this file fits a device: 3 = by id, 2 = by name,
    /// 1 = by class only; None = it doesn't. A class, if given, must always
    /// hold, and the device must have at least one code the file maps.
    fn score(&self, dev: &DeviceInfo) -> Option<u8> {
        let r = &self.rules;
        if r.class.is_some() && r.class != Class::of(dev) {
            return None;
        }
        let maps_something =
            dev.keys.iter().any(|k| self.keys.contains_key(k)) || dev.axes.iter().any(|(a, _, _)| self.axes.contains_key(a));
        if !maps_something {
            return None;
        }
        let name = dev.name.to_lowercase();
        if r.ids.iter().filter_map(|id| parse_id(id)).any(|(v, p)| v == dev.vendor && p.is_none_or(|p| p == dev.product)) {
            Some(3)
        } else if r.names.iter().any(|n| name.contains(&n.to_lowercase())) {
            Some(2)
        } else if r.ids.is_empty() && r.names.is_empty() {
            Some(1)
        } else {
            None
        }
    }

    /// Per-device translation state for a device this file was chosen for.
    pub fn for_device(&self, dev: &DeviceInfo) -> DeviceMap {
        let axes = dev
            .axes
            .iter()
            .filter(|&&(_, min, max)| max > min)
            .filter_map(|&(code, min, max)| {
                let actions = *self.axes.get(&code)?;
                let center = (min as f32 + max as f32) / 2.0;
                let half = (max as f32 - min as f32) / 2.0;
                Some((code, Axis { actions, center, half, deadzone: self.deadzone, dir: 0, analog: half > 1.0, released: None, pos: 0.0 }))
            })
            .collect();
        DeviceMap { keys: self.keys.clone(), axes }
    }
}

fn parse_id(id: &str) -> Option<(u16, Option<u16>)> {
    let (v, p) = match id.split_once(':') {
        Some((v, p)) => (v, Some(p)),
        None => (id, None),
    };
    let v = u16::from_str_radix(v, 16).ok()?;
    match p {
        Some(p) => Some((v, Some(u16::from_str_radix(p, 16).ok()?))),
        None => Some((v, None)),
    }
}

/// The best-fitting mapping for a device; on a tie, the earlier file.
pub fn choose<'a>(mappings: &'a [Mapping], dev: &DeviceInfo) -> Option<&'a Mapping> {
    let mut best: Option<(u8, &Mapping)> = None;
    for m in mappings {
        if let Some(s) = m.score(dev) {
            if best.is_none_or(|(b, _)| s > b) {
                best = Some((s, m));
            }
        }
    }
    best.map(|(_, m)| m)
}

/// The shipped files, each replaced by a same-named file in `user_dir`, plus
/// any other files there. A broken user file is reported and the shipped
/// version (if any) kept, so a typo can't take input away.
pub fn load(user_dir: Option<&Path>, resolve: &impl Fn(&str) -> Option<(u16, u16)>) -> Vec<Mapping> {
    let mut out: Vec<Mapping> = SHIPPED
        .iter()
        .filter_map(|(file, text)| Mapping::parse(file, text, resolve).map_err(|e| eprintln!("[input] {e}")).ok())
        .collect();
    let Some(entries) = user_dir.and_then(|d| std::fs::read_dir(d).ok()) else { return out };
    let mut paths: Vec<_> = entries.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "toml")).collect();
    paths.sort();
    for path in paths {
        let file = path.file_name().unwrap().to_string_lossy().into_owned();
        let parsed = std::fs::read_to_string(&path).map_err(|e| format!("{file}: {e}")).and_then(|t| Mapping::parse(&file, &t, resolve));
        match parsed {
            Ok(m) => match out.iter_mut().find(|o| o.file == m.file) {
                Some(slot) => *slot = m,
                None => out.push(m),
            },
            Err(e) => eprintln!("[input] {} ignored: {e}", path.display()),
        }
    }
    out
}

struct Axis {
    actions: [Action; 2],
    center: f32,
    half: f32,
    deadzone: f32,
    /// -1, 0 or 1: the direction currently pressed.
    dir: i8,
    /// Wider than -1..1. Hats (d-pad) are exact and never rebound.
    analog: bool,
    /// The direction last released and when, for the rebound lockout.
    released: Option<(i8, Instant)>,
    /// Last position seen, -1..1, so a press held back by the lockout can be
    /// fired by `DeviceMap::tick` without waiting for another event.
    pos: f32,
}

/// Turns one device's raw events into action presses (true) and releases (false).
pub struct DeviceMap {
    keys: HashMap<u16, Action>,
    axes: HashMap<u16, Axis>,
}

impl DeviceMap {
    pub fn translate(&mut self, kind: u16, code: u16, value: i32, now: Instant) -> Vec<(Action, bool)> {
        match kind {
            // value 2 is the kernel's own autorepeat; the dispatcher repeats instead.
            EV_KEY if value != 2 => self.keys.get(&code).map(|&a| vec![(a, value == 1)]).unwrap_or_default(),
            EV_ABS => self.axes.get_mut(&code).map(|axis| axis.update(value, now)).unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// Fires any analog press that the rebound lockout held back and whose
    /// window has ended, while the stick is still past the deadzone.
    pub fn tick(&mut self, now: Instant) -> Vec<(Action, bool)> {
        self.axes.values_mut().flat_map(|axis| axis.tick(now)).collect()
    }

    /// When `tick` next has something to do (the earliest lockout end), if any.
    pub fn deadline(&self) -> Option<Instant> {
        self.axes.values().filter_map(Axis::deadline).min()
    }
}

impl Axis {
    fn update(&mut self, value: i32, now: Instant) -> Vec<(Action, bool)> {
        self.pos = (value as f32 - self.center) / self.half;
        self.evaluate(now)
    }

    /// The direction a press is waiting for: the stick is past the deadzone
    /// but nothing is pressed (the lockout suppressed it).
    fn pending(&self) -> Option<i8> {
        if !self.analog || self.dir != 0 {
            return None;
        }
        let dir = if self.pos >= self.deadzone { 1 } else if self.pos <= -self.deadzone { -1 } else { return None };
        self.released.filter(|&(d, _)| d == -dir).map(|_| dir)
    }

    fn tick(&mut self, now: Instant) -> Vec<(Action, bool)> {
        match self.pending() {
            Some(_) => self.evaluate(now),
            None => Vec::new(),
        }
    }

    fn deadline(&self) -> Option<Instant> {
        self.pending().and_then(|_| self.released).map(|(_, at)| at + REBOUND_LOCKOUT)
    }

    fn evaluate(&mut self, now: Instant) -> Vec<(Action, bool)> {
        let pos = self.pos;
        let held = self.dir != 0 && pos * self.dir as f32 >= self.deadzone * RELEASE_FRACTION;
        let dir = if held {
            self.dir
        } else if pos >= self.deadzone {
            1
        } else if pos <= -self.deadzone {
            -1
        } else {
            0
        };
        let mut out = Vec::new();
        if dir != self.dir {
            let was = self.dir;
            if was != 0 {
                out.push((self.action(was), false));
                self.released = Some((was, now));
            }
            let rebound = self.analog
                && dir != 0
                && self.released.is_some_and(|(d, at)| d == -dir && now.saturating_duration_since(at) < REBOUND_LOCKOUT);
            let dir = if rebound { 0 } else { dir };
            if dir != 0 && dir != was {
                out.push((self.action(dir), true));
            }
            self.dir = dir;
        }
        out
    }

    fn action(&self, dir: i8) -> Action {
        self.actions[if dir < 0 { 0 } else { 1 }]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ABS_X: u16 = 0;

    /// A fixed instant for tests that don't care about time.
    fn t0() -> Instant {
        thread_local! { static T0: Instant = Instant::now(); }
        T0.with(|t| *t)
    }
    fn ms(n: u64) -> Instant {
        t0() + Duration::from_millis(n)
    }
    const KEY_UP: u16 = 103;

    /// Enough of the kernel's names for these tests and the shipped files'
    /// structure; devices.rs checks the real names on Linux.
    fn fake_resolve(name: &str) -> Option<(u16, u16)> {
        let code = (name.bytes().map(u32::from).sum::<u32>() % 500) as u16;
        match name {
            "KEY_UP" => Some((EV_KEY, KEY_UP)),
            "KEY_ENTER" => Some((EV_KEY, KEY_ENTER)),
            "BTN_SOUTH" => Some((EV_KEY, BTN_SOUTH)),
            "ABS_X" => Some((EV_ABS, ABS_X)),
            n if n.starts_with("KEY_") || n.starts_with("BTN_") => Some((EV_KEY, code)),
            n if n.starts_with("ABS_") => Some((EV_ABS, code)),
            _ => None,
        }
    }

    fn gamepad(name: &str, vendor: u16) -> DeviceInfo {
        DeviceInfo { name: name.into(), vendor, product: 0x0b12, keys: vec![BTN_SOUTH], axes: vec![(ABS_X, -32768, 32767)] }
    }

    fn shipped() -> Vec<Mapping> {
        load(None, &fake_resolve)
    }

    #[test]
    fn shipped_files_parse() {
        assert_eq!(shipped().len(), SHIPPED.len());
    }

    #[test]
    fn picks_most_specific_file() {
        let m = shipped();
        let file = |dev: &DeviceInfo| choose(&m, dev).map(|m| m.file.as_str());
        assert_eq!(file(&gamepad("Xbox Wireless Controller", 0x045e)), Some("xbox.toml"));
        assert_eq!(file(&gamepad("Generic X-Box pad", 0x1234)), Some("xbox.toml"));
        assert_eq!(file(&gamepad("DualSense Wireless Controller", 0x054c)), Some("playstation.toml"));
        assert_eq!(file(&gamepad("8BitDo Pro 2", 0x2dc8)), Some("gamepad.toml"));
        let cec = DeviceInfo { name: "vc4-hdmi-0".into(), vendor: 0, product: 0, keys: vec![KEY_ENTER, KEY_UP], axes: vec![] };
        assert_eq!(file(&cec), Some("cec-remote.toml"));
        let kbd = DeviceInfo { name: "Razer BlackWidow".into(), vendor: 0x1532, product: 0, keys: vec![KEY_A, KEY_ENTER], axes: vec![] };
        assert_eq!(file(&kbd), Some("keyboard.toml"));
    }

    #[test]
    fn controller_side_devices_are_ignored() {
        // Same vendor and name as the pad, but no face buttons: the motion
        // sensors would otherwise turn tilting the controller into navigation.
        let m = shipped();
        let motion = DeviceInfo {
            name: "DualSense Wireless Controller Motion Sensors".into(),
            vendor: 0x054c,
            product: 0x0ce6,
            keys: vec![],
            axes: vec![(ABS_X, -32768, 32767)],
        };
        assert!(choose(&m, &motion).is_none());
        let jack = DeviceInfo { name: "vc4-hdmi-0 HDMI Jack".into(), vendor: 0, product: 0, keys: vec![], axes: vec![] };
        assert!(choose(&m, &jack).is_none());
        let power = DeviceInfo { name: "pwr_button".into(), vendor: 0, product: 0, keys: vec![116], axes: vec![] };
        assert!(choose(&m, &power).is_none());
    }

    #[test]
    fn keys_press_release_and_skip_autorepeat() {
        let m = Mapping::parse("t.toml", "[match]\nnames = [\"t\"]\n[keys]\nKEY_UP = \"Up\"\n", &fake_resolve).unwrap();
        let mut d = m.for_device(&gamepad("t", 0));
        assert_eq!(d.translate(EV_KEY, KEY_UP, 1, t0()), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_KEY, KEY_UP, 2, t0()), vec![]);
        assert_eq!(d.translate(EV_KEY, KEY_UP, 0, t0()), vec![(Action::Up, false)]);
        assert_eq!(d.translate(EV_KEY, 999, 1, t0()), vec![]);
    }

    #[test]
    fn stick_deadzone_and_hysteresis() {
        let text = "deadzone = 0.4\n[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Left\", \"Right\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut d = m.for_device(&gamepad("t", 0));
        let at = |x: f32| (x * 32767.0) as i32;

        // drift and noise around the centre: nothing, ever
        for v in [0.0, 0.1, -0.2, 0.39, -0.39, 0.05] {
            assert_eq!(d.translate(EV_ABS, ABS_X, at(v), t0()), vec![], "at {v}");
        }
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.5), t0()), vec![(Action::Right, true)]);
        // wobbling just under the deadzone keeps it held
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.32), t0()), vec![]);
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.2), t0()), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.9), t0()), vec![(Action::Right, true)]);
        // A jump straight across releases at once; the opposite press waits out
        // the rebound lockout and fires on the next event after it.
        assert_eq!(d.translate(EV_ABS, ABS_X, at(-0.9), ms(500)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, at(-0.9), ms(700)), vec![(Action::Left, true)]);
    }

    #[test]
    fn tick_fires_held_back_reversal_without_new_events() {
        let text = "deadzone = 0.4\n[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Left\", \"Right\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut d = m.for_device(&gamepad("t", 0));
        let at = |x: f32| (x * 32767.0) as i32;
        assert_eq!(d.deadline(), None);
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.9), ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.deadline(), None);
        // Quick reversal: release now, Left held back; no more events arrive.
        assert_eq!(d.translate(EV_ABS, ABS_X, at(-0.9), ms(100)), vec![(Action::Right, false)]);
        assert_eq!(d.deadline(), Some(ms(250)));
        assert_eq!(d.tick(ms(200)), vec![]);
        assert_eq!(d.tick(ms(250)), vec![(Action::Left, true)]);
        assert_eq!(d.deadline(), None);
        assert_eq!(d.tick(ms(400)), vec![]);
    }

    #[test]
    fn tick_does_nothing_if_stick_returned_to_centre() {
        let text = "deadzone = 0.4\n[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Left\", \"Right\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut d = m.for_device(&gamepad("t", 0));
        let at = |x: f32| (x * 32767.0) as i32;
        d.translate(EV_ABS, ABS_X, at(0.9), ms(0));
        d.translate(EV_ABS, ABS_X, at(-0.9), ms(100));
        assert!(d.deadline().is_some());
        d.translate(EV_ABS, ABS_X, 0, ms(120));
        assert_eq!(d.deadline(), None);
        assert_eq!(d.tick(ms(300)), vec![]);
    }

    #[test]
    fn tick_ignores_hats() {
        let text = "[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Up\", \"Down\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut dev = gamepad("t", 0);
        dev.axes = vec![(ABS_X, -1, 1)];
        let mut d = m.for_device(&dev);
        d.translate(EV_ABS, ABS_X, -1, ms(0));
        d.translate(EV_ABS, ABS_X, 1, ms(10));
        assert_eq!(d.deadline(), None);
        assert_eq!(d.tick(ms(500)), vec![]);
    }

    #[test]
    fn hat_axis_maps_both_ways() {
        let text = "[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Up\", \"Down\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut dev = gamepad("t", 0);
        dev.axes = vec![(ABS_X, -1, 1)];
        let mut d = m.for_device(&dev);
        assert_eq!(d.translate(EV_ABS, ABS_X, -1, t0()), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, 0, t0()), vec![(Action::Up, false)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, 1, t0()), vec![(Action::Down, true)]);
    }

    // A DualSense as hid-playstation reports it: sticks and triggers 0..255,
    // d-pad on the hat axes.
    const DS_USB: &str = "Sony Interactive Entertainment DualSense Wireless Controller";
    const DS_BT: &str = "DualSense Wireless Controller";

    fn code(name: &str) -> u16 {
        fake_resolve(name).unwrap().1
    }

    fn dualsense(name: &str) -> DeviceInfo {
        let keys = ["BTN_SOUTH", "BTN_EAST", "BTN_NORTH", "BTN_WEST", "BTN_START", "BTN_SELECT", "BTN_MODE"].map(code).to_vec();
        let mut axes: Vec<_> = ["ABS_X", "ABS_Y", "ABS_RX", "ABS_RY", "ABS_Z", "ABS_RZ"].map(|n| (code(n), 0, 255)).to_vec();
        axes.extend(["ABS_HAT0X", "ABS_HAT0Y"].map(|n| (code(n), -1, 1)));
        DeviceInfo { name: name.into(), vendor: 0x054c, product: 0x0ce6, keys, axes }
    }

    fn dualsense_map(name: &str) -> DeviceMap {
        let m = shipped();
        let dev = dualsense(name);
        choose(&m, &dev).expect("DualSense picks a file").for_device(&dev)
    }

    #[test]
    fn dualsense_main_node_picks_playstation_over_usb_and_bluetooth() {
        let m = shipped();
        for name in [DS_USB, DS_BT] {
            assert_eq!(choose(&m, &dualsense(name)).map(|m| m.file.as_str()), Some("playstation.toml"), "{name}");
        }
        // The Edge has another product id and the same vendor.
        let mut edge = dualsense("DualSense Edge Wireless Controller");
        edge.product = 0x0df2;
        assert_eq!(choose(&m, &edge).map(|m| m.file.as_str()), Some("playstation.toml"));
    }

    #[test]
    fn dualsense_touchpad_and_motion_nodes_get_no_mapping() {
        let m = shipped();
        let touchpad_keys = ["BTN_LEFT", "BTN_TOUCH", "BTN_TOOL_FINGER", "BTN_TOOL_DOUBLETAP"].map(code).to_vec();
        let mt: Vec<_> = ["ABS_X", "ABS_Y", "ABS_MT_SLOT", "ABS_MT_TRACKING_ID", "ABS_MT_POSITION_X", "ABS_MT_POSITION_Y"]
            .map(|n| (code(n), 0, 1919))
            .to_vec();
        let motion_axes: Vec<_> = ["ABS_X", "ABS_Y", "ABS_Z", "ABS_RX", "ABS_RY", "ABS_RZ"].map(|n| (code(n), -32768, 32767)).to_vec();
        for name in [DS_USB, DS_BT] {
            let touchpad = DeviceInfo { name: format!("{name} Touchpad"), vendor: 0x054c, product: 0x0ce6, keys: touchpad_keys.clone(), axes: mt.clone() };
            let motion = DeviceInfo { name: format!("{name} Motion Sensors"), vendor: 0x054c, product: 0x0ce6, keys: vec![], axes: motion_axes.clone() };
            assert!(choose(&m, &touchpad).is_none(), "{}", touchpad.name);
            assert!(choose(&m, &motion).is_none(), "{}", motion.name);
        }
    }

    #[test]
    fn dualsense_buttons_and_dpad_map_to_actions() {
        let mut d = dualsense_map(DS_BT);
        assert_eq!(d.translate(EV_KEY, code("BTN_MODE"), 1, t0()), vec![(Action::Home, true)]);
        assert_eq!(d.translate(EV_KEY, code("BTN_MODE"), 0, t0()), vec![(Action::Home, false)]);
        assert_eq!(d.translate(EV_KEY, code("BTN_SOUTH"), 1, t0()), vec![(Action::Select, true)]);
        assert_eq!(d.translate(EV_KEY, code("BTN_EAST"), 1, t0()), vec![(Action::Back, true)]);
        assert_eq!(d.translate(EV_KEY, code("BTN_START"), 1, t0()), vec![(Action::Menu, true)]);
        // Square, Triangle and Create do nothing.
        for n in ["BTN_WEST", "BTN_NORTH", "BTN_SELECT"] {
            assert_eq!(d.translate(EV_KEY, code(n), 1, t0()), vec![], "{n}");
        }
        let (hx, hy) = (code("ABS_HAT0X"), code("ABS_HAT0Y"));
        assert_eq!(d.translate(EV_ABS, hx, 1, t0()), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, hx, 0, t0()), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, hx, -1, t0()), vec![(Action::Left, true)]);
        assert_eq!(d.translate(EV_ABS, hx, 0, t0()), vec![(Action::Left, false)]);
        assert_eq!(d.translate(EV_ABS, hy, -1, t0()), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_ABS, hy, 0, t0()), vec![(Action::Up, false)]);
        assert_eq!(d.translate(EV_ABS, hy, 1, t0()), vec![(Action::Down, true)]);
    }

    #[test]
    fn dualsense_stick_at_rest_and_drift_never_presses() {
        let mut d = dualsense_map(DS_USB);
        let (x, y) = (code("ABS_X"), code("ABS_Y"));
        // Resting 120..136 around the 0..255 centre, many updates.
        for i in 0..2000 {
            let v = 120 + (i * 7) % 17;
            assert_eq!(d.translate(EV_ABS, x, v, t0()), vec![], "x {v}");
            assert_eq!(d.translate(EV_ABS, y, 136 - (i * 5) % 17, t0()), vec![], "y at update {i}");
        }
        // A real push still works: 255 is Right, back to rest releases.
        assert_eq!(d.translate(EV_ABS, x, 255, t0()), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, t0()), vec![(Action::Right, false)]);
        // The right stick, triggers and a DualSense without ABS_RX mapping stay silent.
        for n in ["ABS_RX", "ABS_RY", "ABS_Z", "ABS_RZ"] {
            assert_eq!(d.translate(EV_ABS, code(n), 255, t0()), vec![], "{n}");
        }
    }

    #[test]
    fn rejects_bad_files() {
        let bad = [
            "[match]\nnames = [\"t\"]\n[keys]\nKEY_NOPE_ = \"Up\"\nNOT_A_CODE = \"Up\"\n",
            "[match]\nnames = [\"t\"]\n[keys]\nKEY_UP = \"Jump\"\n",
            "[match]\nnames = [\"t\"]\n[keys]\nABS_X = \"Up\"\n",
            "[match]\n",
            "[match]\nids = [\"xbox\"]\n",
            "deadzone = 1.5\n[match]\nnames = [\"t\"]\n",
        ];
        for text in bad {
            assert!(Mapping::parse("t.toml", text, &fake_resolve).is_err(), "accepted: {text}");
        }
    }

    fn flick_map() -> DeviceMap {
        dualsense_map(DS_BT)
    }

    #[test]
    fn stick_flick_overshoot_does_not_fire_the_opposite_direction() {
        let mut d = flick_map();
        let x = code("ABS_X");
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(20)), vec![(Action::Right, false)]);
        // Spring-back overshoot to the left within the lockout: silent.
        assert_eq!(d.translate(EV_ABS, x, 40, ms(40)), vec![]);
        assert_eq!(d.translate(EV_ABS, x, 100, ms(60)), vec![]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(80)), vec![]);
        // Same for a left flick overshooting right.
        assert_eq!(d.translate(EV_ABS, x, 0, ms(1000)), vec![(Action::Left, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(1020)), vec![(Action::Left, false)]);
        assert_eq!(d.translate(EV_ABS, x, 230, ms(1040)), vec![]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(1060)), vec![]);
    }

    #[test]
    fn stick_deliberate_reversal_fires_after_the_lockout() {
        let mut d = flick_map();
        let x = code("ABS_X");
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(20)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(20 + 200)), vec![(Action::Left, true)]);
        // Held past the window: the next event fires it.
        let mut d = flick_map();
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(20)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(60)), vec![]);
        assert_eq!(d.translate(EV_ABS, x, 2, ms(200)), vec![(Action::Left, true)]);
    }

    #[test]
    fn stick_same_direction_and_other_axis_are_not_locked() {
        let mut d = flick_map();
        let (x, y) = (code("ABS_X"), code("ABS_Y"));
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(10)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, x, 255, ms(20)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(30)), vec![(Action::Right, false)]);
        // Y is independent of X's lockout, and has its own.
        assert_eq!(d.translate(EV_ABS, y, 255, ms(40)), vec![(Action::Down, true)]);
        assert_eq!(d.translate(EV_ABS, y, 128, ms(50)), vec![(Action::Down, false)]);
        assert_eq!(d.translate(EV_ABS, y, 0, ms(60)), vec![]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(200)), vec![(Action::Left, true)]);
    }

    #[test]
    fn hat_axis_is_never_locked_out() {
        let mut d = dualsense_map(DS_BT);
        let hx = code("ABS_HAT0X");
        assert_eq!(d.translate(EV_ABS, hx, 1, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, hx, 0, ms(5)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, hx, -1, ms(10)), vec![(Action::Left, true)]);
        // A straight hat flip is instant too.
        assert_eq!(d.translate(EV_ABS, hx, 1, ms(15)), vec![(Action::Left, false), (Action::Right, true)]);
    }


    #[test]
    fn rebound_lockout_edge_is_exclusive_at_exactly_150_ms() {
        let x = code("ABS_X");
        // 149 ms after the release: still locked out.
        let mut d = flick_map();
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(100)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(100 + 149)), vec![]);
        // Exactly 150 ms: the window has ended, so the press fires.
        let mut d = flick_map();
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(100)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(100 + 150)), vec![(Action::Left, true)]);
    }

    #[test]
    fn direct_flip_across_the_stick_is_locked_for_the_new_side() {
        // One event jumps from full right to full left: Right releases, and
        // Left is swallowed as the rebound of that release.
        let mut d = flick_map();
        let x = code("ABS_X");
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(10)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, x, 0, ms(300)), vec![(Action::Left, true)]);
    }

    #[test]
    fn two_axes_keep_independent_lockouts() {
        let mut d = flick_map();
        let (x, y) = (code("ABS_X"), code("ABS_Y"));
        assert_eq!(d.translate(EV_ABS, x, 255, ms(0)), vec![(Action::Right, true)]);
        assert_eq!(d.translate(EV_ABS, x, 128, ms(10)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, y, 0, ms(20)), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_ABS, y, 128, ms(30)), vec![(Action::Up, false)]);
        // Overshoots on both axes inside their own windows are both silent.
        assert_eq!(d.translate(EV_ABS, x, 0, ms(40)), vec![]);
        assert_eq!(d.translate(EV_ABS, y, 255, ms(50)), vec![]);
        // X's window ended (10 + 150), Y's has not (30 + 150).
        assert_eq!(d.translate(EV_ABS, x, 0, ms(165)), vec![(Action::Left, true)]);
        assert_eq!(d.translate(EV_ABS, y, 255, ms(170)), vec![]);
        assert_eq!(d.translate(EV_ABS, y, 255, ms(181)), vec![(Action::Down, true)]);
    }

    #[test]
    fn hat_reversal_inside_150_ms_still_fires_on_the_vertical_hat_too() {
        let mut d = dualsense_map(DS_BT);
        let hy = code("ABS_HAT0Y");
        assert_eq!(d.translate(EV_ABS, hy, -1, ms(0)), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_ABS, hy, 0, ms(1)), vec![(Action::Up, false)]);
        assert_eq!(d.translate(EV_ABS, hy, 1, ms(2)), vec![(Action::Down, true)]);
    }

    fn user_dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("lumen-mapping-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn load_user_file_replaces_shipped_and_adds_new_ones() {
        let dir = user_dir("override");
        // Same name as a shipped file: replaces it (this one only maps BTN_EAST).
        std::fs::write(dir.join("xbox.toml"), "[match]\nids = [\"045e\"]\n[keys]\nBTN_EAST = \"Back\"\n").unwrap();
        std::fs::write(dir.join("mine.toml"), "[match]\nnames = [\"zzz pad\"]\n[keys]\nBTN_SOUTH = \"Select\"\n").unwrap();
        std::fs::write(dir.join("notes.txt"), "[match\n").unwrap();
        let m = load(Some(&dir), &fake_resolve);
        assert_eq!(m.len(), SHIPPED.len() + 1);
        assert!(m.iter().any(|x| x.file == "mine.toml"));
        let mut dev = gamepad("Xbox Wireless Controller", 0x045e);
        dev.keys.push(code("BTN_EAST"));
        let chosen = choose(&m, &dev).unwrap();
        assert_eq!(chosen.file, "xbox.toml");
        let mut d = chosen.for_device(&dev);
        // The replaced xbox.toml maps only BTN_EAST.
        assert_eq!(d.translate(EV_KEY, code("BTN_EAST"), 1, t0()), vec![(Action::Back, true)]);
        assert_eq!(d.translate(EV_KEY, BTN_SOUTH, 1, t0()), vec![]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_broken_user_file_keeps_shipped_version_and_other_files_load() {
        let dir = user_dir("broken");
        std::fs::write(dir.join("xbox.toml"), "[match\nthis is not toml").unwrap();
        std::fs::write(dir.join("typo.toml"), "[match]\nnames = [\"t\"]\n[keys]\nKEY_UP = \"Jump\"\n").unwrap();
        std::fs::write(dir.join("good.toml"), "[match]\nnames = [\"good\"]\n[keys]\nKEY_UP = \"Up\"\n").unwrap();
        let m = load(Some(&dir), &fake_resolve);
        // Shipped five, plus only good.toml.
        assert_eq!(m.len(), SHIPPED.len() + 1);
        assert!(m.iter().any(|x| x.file == "good.toml"));
        assert!(!m.iter().any(|x| x.file == "typo.toml"));
        let dev = gamepad("Xbox Wireless Controller", 0x045e);
        let mut d = choose(&m, &dev).unwrap().for_device(&dev);
        assert_eq!(d.translate(EV_KEY, BTN_SOUTH, 1, t0()), vec![(Action::Select, true)]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn load_with_missing_user_dir_gives_only_shipped_files() {
        let missing = std::env::temp_dir().join(format!("lumen-mapping-nope-{}", std::process::id()));
        assert_eq!(load(Some(&missing), &fake_resolve).len(), SHIPPED.len());
    }
}
