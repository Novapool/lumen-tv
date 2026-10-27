// Power screen actions: Restart Lumen, Reboot, Shut down (ui/power.slint).
//
// Restart Lumen just ends the event loop: lumen.service has Restart=always,
// so the session comes back (under lumen-test.service, ExecStopPost starts
// lumen.service, Lumen's boot session, instead). Reboot and Shut down go
// through logind with `systemctl`: polkit lets the active local session on
// tty1 do that without a password, so no sudoers entry is needed. They run on
// a worker thread and report a failure back to the screen.

use std::time::Duration;

use slint::{ComponentHandle, Timer};

use crate::AppWindow;

/// The `systemctl` verb for a Power screen action, or None for Restart Lumen
/// (handled in-process) and unknown actions.
fn systemctl_verb(action: &str) -> Option<&'static str> {
    match action {
        "reboot" => Some("reboot"),
        "poweroff" => Some("poweroff"),
        _ => None,
    }
}

/// What the screen shows while an action is under way.
fn busy_text(action: &str) -> Option<&'static str> {
    match action {
        "restart-lumen" => Some("Restarting Lumen…"),
        "reboot" => Some("Rebooting…"),
        "poweroff" => Some("Shutting down…"),
        _ => None,
    }
}

pub fn wire(ui: &AppWindow) {
    let weak = ui.as_weak();
    ui.on_power_changed(move |open| {
        println!("[power] {}", if open { "open" } else { "closed" });
        if let Some(ui) = weak.upgrade() {
            // A failure message stays until the screen closes.
            if !open {
                ui.set_power_message("".into());
                ui.set_power_message_level(0);
            }
        }
    });

    let weak = ui.as_weak();
    ui.on_power_action(move |action| {
        let Some(ui) = weak.upgrade() else { return };
        let Some(text) = busy_text(&action) else {
            println!("[power] unknown action {action:?}");
            return;
        };
        println!("[power] {action}");
        ui.set_power_message(text.into());
        ui.set_power_message_level(0);
        let Some(verb) = systemctl_verb(&action) else {
            // Restart Lumen: give the message a frame, then exit.
            Timer::single_shot(Duration::from_millis(300), || {
                let _ = slint::quit_event_loop();
            });
            return;
        };
        if !cfg!(target_os = "linux") {
            println!("[power] (not on the Pi: would run systemctl {verb})");
            return;
        }
        let weak = weak.clone();
        std::thread::spawn(move || {
            let result = std::process::Command::new("systemctl").args(["--no-ask-password", verb]).output();
            let error = match result {
                Ok(out) if out.status.success() => return,
                Ok(out) => {
                    let err = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if err.is_empty() { format!("systemctl {verb} failed ({})", out.status) } else { err }
                }
                Err(e) => format!("systemctl {verb}: {e}"),
            };
            eprintln!("[power] {verb} failed: {error}");
            let _ = weak.upgrade_in_event_loop(move |ui| {
                ui.set_power_message(format!("Couldn't {}: {error}", if verb == "reboot" { "reboot" } else { "shut down" }).into());
                ui.set_power_message_level(-1);
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_map_to_systemctl() {
        assert_eq!(systemctl_verb("reboot"), Some("reboot"));
        assert_eq!(systemctl_verb("poweroff"), Some("poweroff"));
        assert_eq!(systemctl_verb("restart-lumen"), None);
        assert_eq!(systemctl_verb("rm -rf"), None);
    }

    #[test]
    fn every_action_has_busy_text() {
        for a in ["restart-lumen", "reboot", "poweroff"] {
            assert!(busy_text(a).is_some(), "{a}");
        }
        assert!(busy_text("nope").is_none());
    }
}
