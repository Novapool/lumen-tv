// A virtual Xbox controller (uinput) for testing the input layer without
// hardware: plugs in, runs a scripted set of presses, lets the left stick
// drift inside the deadzone, then unplugs. Run it on the Pi while Lumen runs
// with LUMEN_INPUT_DEBUG=1 and compare the journal with what this prints:
//
//   cargo run --release --example fake-pad [drift seconds, default 60]
//   cargo run --release --example fake-pad dualsense [drift seconds, default 60]
//
// `dualsense` plugs in the three nodes a real DualSense makes (pad, Touchpad,
// Motion Sensors; 054c:0ce6, sticks 0..255), runs a Home-screen-safe script,
// then unplugs and re-plugs them like a Bluetooth reconnect.
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
            self.axis_rest(axis, value, 0, what, hold)
        }

        /// Like `axis`, returning to `rest` afterwards (a DualSense stick rests at 128).
        fn axis_rest(&mut self, axis: Abs, value: i32, rest: i32, what: &str, hold: Duration) -> std::io::Result<()> {
            self.step(what);
            self.emit(EventType::ABSOLUTE, axis.0, value)?;
            sleep(hold);
            self.emit(EventType::ABSOLUTE, axis.0, rest)?;
            sleep(Duration::from_millis(400));
            Ok(())
        }
    }

    pub fn run() -> std::io::Result<()> {
        let mut args = std::env::args().skip(1).peekable();
        let dualsense = args.next_if(|a| a == "dualsense").is_some();
        let drift_secs: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(60);
        if dualsense {
            return run_dualsense(drift_secs);
        }

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

    const DS_NAME: &str = "Sony Interactive Entertainment DualSense Wireless Controller";
    const DS_REST: i32 = 128;

    /// The three nodes hid-playstation creates for one DualSense.
    // The nodes are held (not read) so they stay plugged in until dropped.
    #[allow(dead_code)]
    struct DualSense {
        pad: Pad,
        touchpad: VirtualDevice,
        motion: VirtualDevice,
    }

    impl DualSense {
        fn plug_in(t0: Instant) -> std::io::Result<DualSense> {
            let id = InputId::new(BusType::BUS_USB, 0x054c, 0x0ce6, 0x8111);
            let mut keys = AttributeSet::<Key>::new();
            for k in [
                Key::BTN_SOUTH, Key::BTN_EAST, Key::BTN_NORTH, Key::BTN_WEST, Key::BTN_TL, Key::BTN_TR, Key::BTN_TL2, Key::BTN_TR2,
                Key::BTN_SELECT, Key::BTN_START, Key::BTN_MODE, Key::BTN_THUMBL, Key::BTN_THUMBR,
            ] {
                keys.insert(k);
            }
            let stick = AbsInfo::new(DS_REST, 0, 255, 0, 0, 0);
            let trigger = AbsInfo::new(0, 0, 255, 0, 0, 0);
            let hat = AbsInfo::new(0, -1, 1, 0, 0, 0);
            let dev = VirtualDevice::builder()?
                .name(DS_NAME)
                .input_id(id.clone())
                .with_keys(&keys)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, stick))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, stick))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Z, trigger))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RX, stick))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RY, stick))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RZ, trigger))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_HAT0X, hat))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_HAT0Y, hat))?
                .build()?;

            // The touchpad: a pointer device with finger buttons, no gamepad buttons.
            let mut tkeys = AttributeSet::<Key>::new();
            for k in [Key::BTN_LEFT, Key::BTN_TOUCH, Key::BTN_TOOL_FINGER, Key::BTN_TOOL_DOUBLETAP] {
                tkeys.insert(k);
            }
            let (px, py) = (AbsInfo::new(0, 0, 1919, 0, 0, 0), AbsInfo::new(0, 0, 1079, 0, 0, 0));
            let touchpad = VirtualDevice::builder()?
                .name(&format!("{DS_NAME} Touchpad"))
                .input_id(id.clone())
                .with_keys(&tkeys)?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, px))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, py))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_MT_SLOT, AbsInfo::new(0, 0, 1, 0, 0, 0)))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_MT_TRACKING_ID, AbsInfo::new(0, -1, 65535, 0, 0, 0)))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_MT_POSITION_X, px))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_MT_POSITION_Y, py))?
                .build()?;

            // Motion sensors: accelerometer and gyro axes only, no keys.
            let accel = AbsInfo::new(0, -32768, 32767, 16, 0, 8192);
            let gyro = AbsInfo::new(0, -2_097_152, 2_097_151, 16, 0, 1024);
            let motion = VirtualDevice::builder()?
                .name(&format!("{DS_NAME} Motion Sensors"))
                .input_id(id.clone())
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_X, accel))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Y, accel))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_Z, accel))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RX, gyro))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RY, gyro))?
                .with_absolute_axis(&UinputAbsSetup::new(Abs::ABS_RZ, gyro))?
                .build()?;
            Ok(DualSense { pad: Pad { dev, t0 }, touchpad, motion })
        }
    }

    fn run_dualsense(drift_secs: u64) -> std::io::Result<()> {
        let t0 = Instant::now();
        let mut ds = DualSense::plug_in(t0)?;
        ds.pad.step("plugged in: pad + Touchpad + Motion Sensors (054c:0ce6)");
        sleep(Duration::from_secs(2));

        // Start this in Settings (Lumen in front). Nothing here presses
        // Select (Cross would launch a tile on Home or toggle a Settings
        // row), so the script only moves focus and exercises Back/Home.
        let tap = Duration::from_millis(80);
        ds.pad.axis(Abs::ABS_HAT0Y, 1, "d-pad Down (expect Down)", tap)?;
        ds.pad.tap(Key::BTN_START, "Options (expect Menu)", tap)?;
        ds.pad.tap(Key::BTN_EAST, "Circle tap (expect Back)", tap)?;
        ds.pad.tap(Key::BTN_EAST, "Circle held 1.2 s (expect Home at 0.8 s, no Back)", Duration::from_millis(1200))?;
        ds.pad.tap(Key::BTN_MODE, "PS button (expect Home)", tap)?;
        ds.pad.axis_rest(Abs::ABS_X, 255, DS_REST, "stick Right flick (expect Right)", tap)?;

        // Drift: wander inside +-30% of the half range (deadzone 40%), 20
        // updates a second, with the motion sensors jittering alongside
        // (expect nothing from either).
        ds.pad.step(&format!("stick drifting inside the deadzone for {drift_secs} s, motion sensors jittering (expect nothing)"));
        let mut seed: u32 = 0x2545_f491;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            (seed % 1000) as f32 / 1000.0 * 2.0 - 1.0
        };
        let wander = |n: f32| DS_REST + (n * 0.3 * 127.5) as i32;
        let end = Instant::now() + Duration::from_secs(drift_secs);
        while Instant::now() < end {
            let (x, y) = (wander(noise()), wander(noise()));
            ds.pad.emit(EventType::ABSOLUTE, Abs::ABS_X.0, x)?;
            ds.pad.emit(EventType::ABSOLUTE, Abs::ABS_Y.0, y)?;
            ds.motion.emit(&[
                InputEvent::new(EventType::ABSOLUTE.0, Abs::ABS_X.0, (noise() * 2000.0) as i32),
                InputEvent::new(EventType::ABSOLUTE.0, Abs::ABS_RZ.0, (noise() * 20000.0) as i32),
            ])?;
            sleep(Duration::from_millis(50));
        }
        ds.pad.emit(EventType::ABSOLUTE, Abs::ABS_X.0, DS_REST)?;
        ds.pad.emit(EventType::ABSOLUTE, Abs::ABS_Y.0, DS_REST)?;

        ds.pad.step("unplugging all three nodes (expect \"removed\" x3)");
        drop(ds);
        sleep(Duration::from_secs(3));

        let mut ds = DualSense::plug_in(t0)?;
        ds.pad.step("re-plugged (a Bluetooth reconnect)");
        sleep(Duration::from_secs(2));
        ds.pad.tap(Key::BTN_MODE, "PS button after reconnect (expect Home)", tap)?;
        ds.pad.step("unplugging");
        Ok(())
    }
}
