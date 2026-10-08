// A virtual Xbox controller (uinput) for testing the input layer without
// hardware: plugs in, runs a scripted set of presses, lets the left stick
// drift inside the deadzone, then unplugs. Run it on the Pi while Lumen runs
// with LUMEN_INPUT_DEBUG=1 and compare the journal with what this prints:
//
//   cargo run --release --example fake-pad [drift seconds, default 60]
//
// Needs write access to /dev/uinput (the input group on Pi OS).

#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("fake-pad needs Linux (uinput)");
}

#[cfg(target_os = "linux")]
fn main() -> std::io::Result<()> {
    pad::run()
}

#[cfg(target_os = "linux")]
mod pad {
    use std::thread::sleep;
    use std::time::{Duration, Instant};

    use evdev::uinput::VirtualDevice;
    use evdev::{AbsInfo, AbsoluteAxisCode as Abs, AttributeSet, BusType, EventType, InputEvent, InputId, KeyCode as Key, UinputAbsSetup};

    const STICK: i32 = 32767;

    struct Pad {
        dev: VirtualDevice,
        t0: Instant,
    }

    impl Pad {
        fn emit(&mut self, kind: EventType, code: u16, value: i32) -> std::io::Result<()> {
            self.dev.emit(&[InputEvent::new(kind.0, code, value)])
        }

        fn step(&self, what: &str) {
            println!("{:6.2}s  {what}", self.t0.elapsed().as_secs_f64());
        }

        fn tap(&mut self, key: Key, what: &str, hold: Duration) -> std::io::Result<()> {
            self.step(what);
            self.emit(EventType::KEY, key.0, 1)?;
            sleep(hold);
            self.emit(EventType::KEY, key.0, 0)?;
            sleep(Duration::from_millis(400));
            Ok(())
        }

        fn axis(&mut self, axis: Abs, value: i32, what: &str, hold: Duration) -> std::io::Result<()> {
            self.step(what);
            self.emit(EventType::ABSOLUTE, axis.0, value)?;
            sleep(hold);
            self.emit(EventType::ABSOLUTE, axis.0, 0)?;
            sleep(Duration::from_millis(400));
            Ok(())
        }
    }

    pub fn run() -> std::io::Result<()> {
        let drift_secs: u64 = std::env::args().nth(1).and_then(|a| a.parse().ok()).unwrap_or(60);

        let mut keys = AttributeSet::<Key>::new();
        for k in [Key::BTN_SOUTH, Key::BTN_EAST, Key::BTN_NORTH, Key::BTN_WEST, Key::BTN_START, Key::BTN_SELECT, Key::BTN_MODE] {
            keys.insert(k);
        }
        let stick = AbsInfo::new(0, -STICK - 1, STICK, 16, 128, 0);
        let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
        let dev = VirtualDevice::builder()?
            .name("Xbox Wireless Controller")
            .input_id(InputId::new(BusType::BUS_USB, 0x045e, 0x0b12, 1))
            .with_keys(&keys)?
            .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, stick))?
            .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_HAT0X, hat))?
            .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_HAT0Y, hat))?
            .build()?;
        let mut pad = Pad { dev, t0: Instant::now() };
        pad.step("plugged in");
        sleep(Duration::from_secs(2));

        let tap = Duration::from_millis(80);
        pad.axis(Abs::ABS_HAT0X, 1, "d-pad Right (expect Right)", tap)?;
        pad.axis(Abs::ABS_HAT0X, -1, "d-pad Left (expect Left)", tap)?;
        pad.axis(Abs::ABS_HAT0Y, 1, "d-pad Down (expect Down)", tap)?;
        pad.tap(Key::BTN_SOUTH, "A (expect Select)", tap)?;
        pad.tap(Key::BTN_START, "Menu (expect Menu)", tap)?;
        pad.tap(Key::BTN_EAST, "B tap (expect Back)", tap)?;
        pad.tap(Key::BTN_EAST, "B held 1.2 s (expect Home at 0.8 s, no Back)", Duration::from_millis(1200))?;
        pad.tap(Key::BTN_MODE, "Xbox button (expect Home)", tap)?;
        pad.tap(Key::BTN_NORTH, "Y (unmapped, expect nothing)", tap)?;
        pad.axis(Abs::ABS_X, STICK, "stick Right held 1.5 s (expect Right, then repeats speeding up)", Duration::from_millis(1500))?;
        pad.axis(Abs::ABS_Y, -STICK, "stick Up flick (expect Up)", tap)?;

        // Drift: wander inside ±30% (deadzone 40%), 20 updates a second.
        pad.step(&format!("stick drifting inside the deadzone for {drift_secs} s (expect nothing)"));
        let mut seed: u32 = 0x2545_f491;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 1000) as f32 / 1000.0 * 2.0 - 1.0
        };
        let end = Instant::now() + Duration::from_secs(drift_secs);
        while Instant::now() < end {
            let (x, y) = ((noise() * 0.3 * STICK as f32) as i32, (noise() * 0.3 * STICK as f32) as i32);
            pad.emit(EventType::ABSOLUTE, Abs::ABS_X.0, x)?;
            pad.emit(EventType::ABSOLUTE, Abs::ABS_Y.0, y)?;
            sleep(Duration::from_millis(50));
        }
        pad.emit(EventType::ABSOLUTE, Abs::ABS_X.0, 0)?;
        pad.emit(EventType::ABSOLUTE, Abs::ABS_Y.0, 0)?;

        pad.step("unplugging (expect \"removed\")");
        Ok(())
    }
}
