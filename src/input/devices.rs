// Linux side of the input layer. Opens every /dev/input/event* node (never
// grabbing it), follows hotplug with inotify, and runs one blocking reader
// thread per device. Everything goes into one channel, read by the input
// thread, which owns the mappings and the dispatcher and calls back with
// each action.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

use evdev::raw_stream::RawDevice;
use evdev::{AbsoluteAxisCode, EventType, KeyCode};
use inotify::{Inotify, WatchMask};

use super::dispatch::{back_long_press_is_home, home_needs_hold, Dispatcher, SourceClass};
use super::mapping::{self, DeviceInfo, DeviceMap, EV_ABS, EV_KEY};
use super::{Action, DeviceStatus, Status};

const INPUT_DIR: &str = "/dev/input";

enum Msg {
    Added(PathBuf, DeviceInfo),
    Event(PathBuf, u16, u16, i32),
    Removed(PathBuf),
}

/// Paths with a reader thread running, so hotplug events don't open a device twice.
type Open = Arc<Mutex<HashSet<PathBuf>>>;

/// Starts reading input. `on_action` runs on the input thread with each
/// action and the name of the device it came from. `status` is kept current
/// for Settings; `debug` (toggleable) turns on recording of raw key names.
pub fn start(
    status: Status,
    debug: Arc<AtomicBool>,
    on_action: impl Fn(Action, &str) + Send + 'static,
) {
    let user_dir = config_dir().map(|d| d.join("input"));
    let mappings = mapping::load(user_dir.as_deref(), &resolve);
    let (tx, rx) = mpsc::channel();
    let open: Open = Arc::default();

    // Watch before the first scan so a device plugged in between isn't missed.
    // ATTRIB matters: udev creates the node first and only then gives it the
    // input group, so the first open can fail with EACCES.
    let watcher = Inotify::init().and_then(|w| {
        w.watches()
            .add(INPUT_DIR, WatchMask::CREATE | WatchMask::ATTRIB)
            .map(|_| w)
    });
    if let Ok(entries) = std::fs::read_dir(INPUT_DIR) {
        for entry in entries.flatten() {
            try_open(entry.path(), &tx, &open);
        }
    }
    match watcher {
        Ok(mut watcher) => {
            let tx = tx.clone();
            let open = open.clone();
            thread::spawn(move || {
                let mut buf = [0; 4096];
                while let Ok(events) = watcher.read_events_blocking(&mut buf) {
                    for name in events.filter_map(|e| e.name) {
                        try_open(Path::new(INPUT_DIR).join(name), &tx, &open);
                    }
                }
                eprintln!("[input] hotplug watch stopped");
            });
        }
        Err(e) => eprintln!("[input] no hotplug ({e}); only devices present at startup work"),
    }
    drop(tx);
    thread::spawn(move || run(rx, mappings, status, debug, on_action));
}

fn config_dir() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("lumen-tv"))
}

/// Resolves a kernel code name ("KEY_UP", "BTN_SOUTH", "ABS_X") for the mapping files.
fn resolve(name: &str) -> Option<(u16, u16)> {
    if let Ok(key) = name.parse::<KeyCode>() {
        return Some((EV_KEY, key.0));
    }
    name.parse::<AbsoluteAxisCode>()
        .ok()
        .map(|axis| (EV_ABS, axis.0))
}

fn try_open(path: PathBuf, tx: &Sender<Msg>, open: &Open) {
    if !path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().starts_with("event"))
    {
        return;
    }
    if !open.lock().unwrap().insert(path.clone()) {
        return;
    }
    let dev = match RawDevice::open(&path) {
        Ok(dev) => dev,
        Err(_) => {
            // Usually EACCES before udev sets permissions; the ATTRIB event retries.
            open.lock().unwrap().remove(&path);
            return;
        }
    };
    let id = dev.input_id();
    let axes = dev
        .get_absinfo()
        .map(|it| {
            it.map(|(code, info)| (code.0, info.minimum(), info.maximum()))
                .collect()
        })
        .unwrap_or_default();
    let info = DeviceInfo {
        name: dev.name().unwrap_or("unnamed").to_string(),
        vendor: id.vendor(),
        product: id.product(),
        keys: dev
            .supported_keys()
            .map(|k| k.iter().map(|k| k.0).collect())
            .unwrap_or_default(),
        axes,
    };
    let _ = tx.send(Msg::Added(path.clone(), info));
    let tx = tx.clone();
    let open = open.clone();
    thread::spawn(move || read(dev, path, tx, open));
}

fn read(mut dev: RawDevice, path: PathBuf, tx: Sender<Msg>, open: Open) {
    'outer: loop {
        match dev.fetch_events() {
            Ok(events) => {
                for ev in events {
                    let kind = ev.event_type();
                    if (kind == EventType::KEY || kind == EventType::ABSOLUTE)
                        && tx
                            .send(Msg::Event(path.clone(), kind.0, ev.code(), ev.value()))
                            .is_err()
                    {
                        break 'outer;
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(_) => break, // ENODEV: unplugged or disconnected
        }
    }
    open.lock().unwrap().remove(&path);
    let _ = tx.send(Msg::Removed(path));
}

struct Device {
    name: String,
    map: DeviceMap,
    class: SourceClass,
}

/// Edits the device's status entry (if it has one) under the lock.
fn with_status(status: &Status, path: &Path, f: impl FnOnce(&mut DeviceStatus)) {
    if let Ok(mut all) = status.lock() {
        let key = path.display().to_string();
        if let Some(d) = all.iter_mut().find(|d| d.path == key) {
            f(d);
        }
    }
}

fn run(
    rx: mpsc::Receiver<Msg>,
    mappings: Vec<mapping::Mapping>,
    status: Status,
    debug: Arc<AtomicBool>,
    on_action: impl Fn(Action, &str),
) {
    let mut devices: HashMap<PathBuf, Device> = HashMap::new();
    let mut dispatch: Dispatcher<std::path::PathBuf> = Dispatcher::default();
    loop {
        let msg = match dispatch.deadline() {
            Some(at) => rx.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
        };
        let now = Instant::now();
        // Due repeats and long presses first, so they keep their order with
        // a release that arrived after their deadline.
        for (path, action) in dispatch.tick(now) {
            if let Some(d) = devices.get(&path) {
                with_status(&status, &path, |s| s.last_action = Some((action, now)));
                on_action(action, &d.name);
            }
        }
        if dispatch.take_held_home_fired() {
            println!("[input] home (held PS in steamlink)");
        }
        match msg {
            Ok(Msg::Added(path, info)) => {
                let chosen = mapping::choose(&mappings, &info);
                if let Ok(mut all) = status.lock() {
                    all.retain(|d| d.path != path.display().to_string());
                    all.push(DeviceStatus {
                        path: path.display().to_string(),
                        name: info.name.clone(),
                        mapping: chosen.map(|m| m.file.clone()),
                        last_action: None,
                        last_raw: None,
                    });
                }
                match chosen {
                    Some(m) => {
                        println!(
                            "[input] {} \"{}\" ({:04x}:{:04x}) → {}",
                            path.display(),
                            info.name,
                            info.vendor,
                            info.product,
                            m.file
                        );
                        devices.insert(
                            path,
                            Device {
                                map: m.for_device(&info),
                                class: if mapping::is_gamepad(&info) { SourceClass::Gamepad } else { SourceClass::Other },
                                name: info.name,
                            },
                        );
                    }
                    None => println!(
                        "[input] {} \"{}\": no mapping, ignored",
                        path.display(),
                        info.name
                    ),
                }
            }
            Ok(Msg::Event(path, kind, code, value)) => {
                if kind == EV_KEY && value == 1 && debug.load(Ordering::Relaxed) {
                    // Raw name, mapped or not: helps when working out a remote.
                    let raw = format!("{:?}", KeyCode::new(code));
                    with_status(&status, &path, |s| s.last_raw = Some((raw, now)));
                }
                let Some(d) = devices.get_mut(&path) else {
                    continue;
                };
                for (action, pressed) in d.map.translate(kind, code, value) {
                    let out = if pressed {
                        // One foreground lookup per press decides both rules.
                        let fg = super::foreground();
                        let hold = action == Action::Home && home_needs_hold(d.class, fg.as_deref());
                        let back_home = back_long_press_is_home(d.class, fg.as_deref());
                        dispatch.press_rules(&path, action, now, hold, back_home)
                    } else {
                        dispatch.release(&path, action, now)
                    };
                    if let Some(action) = out {
                        with_status(&status, &path, |s| s.last_action = Some((action, now)));
                        on_action(action, &d.name);
                    }
                }
            }
            Ok(Msg::Removed(path)) => {
                dispatch.release_device(&path);
                if let Ok(mut all) = status.lock() {
                    all.retain(|d| d.path != path.display().to_string());
                }
                if let Some(d) = devices.remove(&path) {
                    println!("[input] {} \"{}\" removed", path.display(), d.name);
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shipped_files_use_real_kernel_names() {
        for (file, text) in mapping::SHIPPED {
            if let Err(e) = mapping::Mapping::parse(file, text, &resolve) {
                panic!("{e}");
            }
        }
    }

    #[test]
    fn shipped_cec_remote_maps_red_enter_and_back() {
        use super::super::mapping::{choose, load, DeviceInfo};
        let mappings = load(None, &resolve);
        let dev = DeviceInfo {
            name: "vc4-hdmi-0".into(),
            vendor: 0,
            product: 0,
            keys: vec![resolve("KEY_ENTER").unwrap().1],
            axes: vec![],
        };
        let m = choose(&mappings, &dev).expect("a mapping for the CEC remote");
        assert_eq!(m.file, "cec-remote.toml");
        let mut map = m.for_device(&dev);
        let key = |name: &str| {
            let (kind, code) = resolve(name).unwrap_or_else(|| panic!("{name} does not resolve"));
            assert_eq!(kind, EV_KEY);
            code
        };
        assert_eq!(
            map.translate(EV_KEY, key("KEY_RED"), 1),
            vec![(Action::Home, true)]
        );
        assert_eq!(
            map.translate(EV_KEY, key("KEY_ENTER"), 1),
            vec![(Action::Select, true)]
        );
        assert_eq!(
            map.translate(EV_KEY, key("KEY_BACK"), 1),
            vec![(Action::Back, true)]
        );
        assert_eq!(
            map.translate(EV_KEY, key("KEY_RED"), 0),
            vec![(Action::Home, false)]
        );
        // Unremapped kernel codes still work if the remap failed.
        assert_eq!(
            map.translate(EV_KEY, key("KEY_OK"), 1),
            vec![(Action::Select, true)]
        );
        assert_eq!(
            map.translate(EV_KEY, key("KEY_EXIT"), 1),
            vec![(Action::Back, true)]
        );
    }
}
