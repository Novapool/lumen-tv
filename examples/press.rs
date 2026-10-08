// A virtual keyboard "lumen-press" (uinput) that taps named keys, for gate
// trials over SSH. Prints the epoch ms of each tap.
//
//   press KEY_RIGHT KEY_RIGHT KEY_F1 KEY_ESC:900     (":ms" holds the key)
//
// Needs write access to /dev/uinput (the input group on Pi OS).

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("press needs Linux (uinput)");
}

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    kb::run()
}

#[cfg(target_os = "linux")]
mod kb {
    use std::str::FromStr;
    use std::thread::sleep;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode as Key};

    fn emit(dev: &mut VirtualDevice, key: Key, value: i32) -> std::io::Result<()> {
        // emit() appends the SYN_REPORT.
        dev.emit(&[InputEvent::new(EventType::KEY.0, key.0, value)])
    }

    pub fn run() -> std::io::Result<()> {
        let mut script: Vec<(String, Key, u64)> = Vec::new();
        for a in std::env::args().skip(1) {
            let (name, hold) = match a.split_once(':') {
                Some((n, h)) => (n.to_string(), h.parse().unwrap_or(60)),
                None => (a.clone(), 60),
            };
            match Key::from_str(&name) {
                Ok(k) => script.push((name, k, hold)),
                Err(_) => {
                    eprintln!("unknown key {name:?}");
                    std::process::exit(2);
                }
            }
        }

        let mut keys = AttributeSet::<Key>::new();
        // Letters + Enter make evdev's class detection say "keyboard".
        for k in 16..=25u16 { keys.insert(Key::new(k)); } // Q..P
        for k in 30..=38u16 { keys.insert(Key::new(k)); } // A..L
        for k in 44..=50u16 { keys.insert(Key::new(k)); } // Z..M
        for k in [Key::KEY_ENTER, Key::KEY_UP, Key::KEY_DOWN, Key::KEY_LEFT, Key::KEY_RIGHT, Key::KEY_F1, Key::KEY_ESC] {
            keys.insert(k);
        }
        for (_, k, _) in &script { keys.insert(*k); }

        let mut dev = VirtualDevice::builder()?
            .name("lumen-press")
            .input_id(InputId::new(BusType::BUS_USB, 0x1209, 0x0001, 1))
            .with_keys(&keys)?
            .build()?;
        sleep(Duration::from_millis(1500)); // let Lumen's inotify hotplug open it
        for (name, key, hold) in script {
            emit(&mut dev, key, 1)?;
            let ms = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis();
            sleep(Duration::from_millis(hold));
            emit(&mut dev, key, 0)?;
            println!("{ms} pressed {name}");
            sleep(Duration::from_millis(300));
        }
        Ok(())
    }
}
