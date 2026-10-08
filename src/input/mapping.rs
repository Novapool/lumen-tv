// Mapping files (config/input/*.toml, overridable per file name in
// ~/.config/lumen-tv/input/): which file a device uses, and how its raw evdev
// events become action presses and releases. Codes are plain numbers here, so
// this is pure logic; devices.rs resolves the code names in the files.

use std::collections::HashMap;
use std::path::Path;

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
                Some((code, Axis { actions, center, half, deadzone: self.deadzone, dir: 0 }))
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
}

/// Turns one device's raw events into action presses (true) and releases (false).
pub struct DeviceMap {
    keys: HashMap<u16, Action>,
    axes: HashMap<u16, Axis>,
}

impl DeviceMap {
    pub fn translate(&mut self, kind: u16, code: u16, value: i32) -> Vec<(Action, bool)> {
        match kind {
            // value 2 is the kernel's own autorepeat; the dispatcher repeats instead.
            EV_KEY if value != 2 => self.keys.get(&code).map(|&a| vec![(a, value == 1)]).unwrap_or_default(),
            EV_ABS => self.axes.get_mut(&code).map(|axis| axis.update(value)).unwrap_or_default(),
            _ => Vec::new(),
        }
    }
}

impl Axis {
    fn update(&mut self, value: i32) -> Vec<(Action, bool)> {
        let pos = (value as f32 - self.center) / self.half;
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
            if self.dir != 0 {
                out.push((self.action(self.dir), false));
            }
            if dir != 0 {
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
        assert_eq!(d.translate(EV_KEY, KEY_UP, 1), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_KEY, KEY_UP, 2), vec![]);
        assert_eq!(d.translate(EV_KEY, KEY_UP, 0), vec![(Action::Up, false)]);
        assert_eq!(d.translate(EV_KEY, 999, 1), vec![]);
    }

    #[test]
    fn stick_deadzone_and_hysteresis() {
        let text = "deadzone = 0.4\n[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Left\", \"Right\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut d = m.for_device(&gamepad("t", 0));
        let at = |x: f32| (x * 32767.0) as i32;

        // drift and noise around the centre: nothing, ever
        for v in [0.0, 0.1, -0.2, 0.39, -0.39, 0.05] {
            assert_eq!(d.translate(EV_ABS, ABS_X, at(v)), vec![], "at {v}");
        }
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.5)), vec![(Action::Right, true)]);
        // wobbling just under the deadzone keeps it held
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.32)), vec![]);
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.2)), vec![(Action::Right, false)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, at(0.9)), vec![(Action::Right, true)]);
        // flick straight across
        assert_eq!(d.translate(EV_ABS, ABS_X, at(-0.9)), vec![(Action::Right, false), (Action::Left, true)]);
    }

    #[test]
    fn hat_axis_maps_both_ways() {
        let text = "[match]\nnames = [\"t\"]\n[axes]\nABS_X = [\"Up\", \"Down\"]\n";
        let m = Mapping::parse("t.toml", text, &fake_resolve).unwrap();
        let mut dev = gamepad("t", 0);
        dev.axes = vec![(ABS_X, -1, 1)];
        let mut d = m.for_device(&dev);
        assert_eq!(d.translate(EV_ABS, ABS_X, -1), vec![(Action::Up, true)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, 0), vec![(Action::Up, false)]);
        assert_eq!(d.translate(EV_ABS, ABS_X, 1), vec![(Action::Down, true)]);
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
}
