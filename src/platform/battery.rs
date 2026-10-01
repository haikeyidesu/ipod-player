//! Small macOS battery adapter. No shell invocation and no UI-thread subprocesses.
use std::{process::Command, time::Duration};

use crate::AppWindow;

#[derive(Debug, PartialEq)]
struct BatteryStatus {
    percent: i32,
    charging: bool,
}

fn parse_status(output: &str) -> Option<BatteryStatus> {
    // Ignore the power-source heading; only a battery entry contains a percentage.
    let line = output.lines().find(|line| line.contains('%'))?;
    let (before_percent, after_percent) = line.split_once('%')?;
    let percent = before_percent
        .split_whitespace()
        .last()?
        .parse::<i32>()
        .ok()?;
    if !(0..=100).contains(&percent) {
        return None;
    }
    // Match a whole field: "discharging" must not count as "charging".
    let charging = after_percent
        .split(';')
        .any(|field| field.trim() == "charging");
    Some(BatteryStatus { percent, charging })
}

fn refresh(app: slint::Weak<AppWindow>) {
    std::thread::spawn(move || {
        let status = Command::new("/usr/bin/pmset")
            .args(["-g", "batt"])
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| parse_status(&String::from_utf8_lossy(&output.stdout)));
        // Slint properties are updated only on the event-loop thread.
        let _ = app.upgrade_in_event_loop(move |app| {
            app.set_battery_level(status.as_ref().map_or(-1, |status| status.percent));
            app.set_battery_charging(status.is_some_and(|status| status.charging));
        });
    });
}

/// Keep the returned timer alive for the application's lifetime.
pub fn start(app: slint::Weak<AppWindow>) -> slint::Timer {
    refresh(app.clone());
    let timer = slint::Timer::default();
    timer.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(60),
        move || {
            refresh(app.clone());
        },
    );
    timer
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discharging_and_charging() {
        assert_eq!(
            parse_status(
                "Now drawing from 'Battery Power'\n -InternalBattery-0 (id=123)\t15%; discharging; 0:19 remaining present: true"
            ),
            Some(BatteryStatus {
                percent: 15,
                charging: false
            })
        );
        assert_eq!(
            parse_status(
                " -InternalBattery-0 (id=123)\t62%; charging; 1:00 remaining present: true"
            ),
            Some(BatteryStatus {
                percent: 62,
                charging: true
            })
        );
    }

    #[test]
    fn handles_full_missing_and_invalid_data() {
        assert_eq!(
            parse_status(
                " -InternalBattery-0 (id=123)\t100%; charged; 0:00 remaining present: true"
            ),
            Some(BatteryStatus {
                percent: 100,
                charging: false
            })
        );
        assert_eq!(parse_status("Now drawing from 'AC Power'"), None);
        assert_eq!(parse_status("battery 101%; charging;"), None);
        assert_eq!(parse_status("battery bad%; charging;"), None);
    }
}
