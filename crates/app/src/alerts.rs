//! Alerts: a desktop notification when something needs attention.
//!
//! Every tick the snapshot is checked against the limits set in
//! Preferences. A value must stay past its limit for a while before an alert
//! is sent (a short spike is not news), an alert is sent once, and it is
//! withdrawn when the value has come back a margin below the limit, so a
//! value that hovers at the limit does not send a stream of them.

use crate::format;
use crate::settings::{AlertSettings, AppSettings};
use corewatch_core::Snapshot;
use gtk::gio;
use gtk::prelude::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

/// One condition's state: past its limit since when, and whether its alert
/// is out.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Trigger {
    since: Option<Instant>,
    sent: bool,
}

/// What a [`Trigger`] step asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum Change {
    Send,
    Withdraw,
}

impl Trigger {
    /// One tick. `past`: the value is beyond the limit; `back`: it is back
    /// a margin on the right side of it; `hold`: how long it must stay past.
    pub fn step(&mut self, past: bool, back: bool, hold: Duration, now: Instant) -> Option<Change> {
        if past {
            let since = *self.since.get_or_insert(now);
            if !self.sent && now.saturating_duration_since(since) >= hold {
                self.sent = true;
                return Some(Change::Send);
            }
            return None;
        }
        self.since = None;
        if self.sent && back {
            self.sent = false;
            return Some(Change::Withdraw);
        }
        None
    }
}

/// How long a value stays past its limit before an alert.
const HOLD_TEMPERATURE: Duration = Duration::from_secs(10);
const HOLD_MEMORY: Duration = Duration::from_secs(30);
const HOLD_DISK: Duration = Duration::ZERO;
const HOLD_PROCESS: Duration = Duration::from_secs(5 * 60);
/// How far back a value must come before its alert goes.
const MARGIN_CELSIUS: f32 = 5.0;
const MARGIN_SHARE: f32 = 0.05;
/// A process "keeps a core busy" at this share of one core.
const BUSY_CORE: f32 = 0.9;

thread_local! {
    static STATE: RefCell<State> = RefCell::default();
}

#[derive(Default)]
struct State {
    triggers: HashMap<String, Trigger>,
}

/// Check one snapshot and send or withdraw notifications. Called once per
/// tick from the window's update loop.
pub fn check(app: &gio::Application, snapshot: &Snapshot) {
    let settings = AppSettings::load_cached().alerts;
    if !settings.enabled {
        STATE.with(|s| withdraw_all(app, &mut s.borrow_mut()));
        return;
    }
    let now = Instant::now();
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let mut seen = HashSet::new();
        for alert in alerts(snapshot, &settings) {
            seen.insert(alert.id.clone());
            let trigger = state.triggers.entry(alert.id.clone()).or_default();
            match trigger.step(alert.past, alert.back, alert.hold, now) {
                Some(Change::Send) => send(app, &alert),
                Some(Change::Withdraw) => app.withdraw_notification(&alert.id),
                None => {}
            }
        }
        // Conditions that went away (a process ended, a disk unmounted).
        state.triggers.retain(|id, trigger| {
            let keep = seen.contains(id);
            if !keep && trigger.sent {
                app.withdraw_notification(id);
            }
            keep
        });
    });
}

fn withdraw_all(app: &gio::Application, state: &mut State) {
    for (id, trigger) in state.triggers.drain() {
        if trigger.sent {
            app.withdraw_notification(&id);
        }
    }
}

/// One condition this tick.
struct Alert {
    /// Notification id: one per condition, so a new one replaces the old.
    id: String,
    title: String,
    body: String,
    /// The page the notification opens.
    page: &'static str,
    past: bool,
    back: bool,
    hold: Duration,
}

fn alerts(s: &Snapshot, limits: &AlertSettings) -> Vec<Alert> {
    let mut list = Vec::new();
    let temperature = |id: &str, what: &str, page: &'static str, celsius: f32| Alert {
        id: id.to_owned(),
        title: format::fmt("{what} is hot", &[("what", &what)]),
        body: format::fmt(
            "{what} is at {temp}, above the limit of {limit}.",
            &[
                ("what", &what),
                ("temp", &format::celsius(celsius)),
                ("limit", &format::celsius(limits.temperature_c)),
            ],
        ),
        page,
        past: celsius >= limits.temperature_c,
        back: celsius < limits.temperature_c - MARGIN_CELSIUS,
        hold: HOLD_TEMPERATURE,
    };
    if let Some(celsius) = s.sensors.as_ref().and_then(|s| s.cpu_celsius()) {
        list.push(temperature(
            "cpu-temperature",
            "The processor",
            "cpu",
            celsius,
        ));
    }
    if let Some(gpu) = &s.gpu {
        for device in &gpu.devices {
            if let Some(celsius) = device.temp_c {
                list.push(temperature(
                    &format!("gpu-temperature-{}", device.id),
                    &device.name,
                    "gpu",
                    celsius as f32,
                ));
            }
        }
    }
    if let Some(m) = &s.memory {
        let used = m.used_fraction();
        list.push(Alert {
            id: "memory".to_owned(),
            title: String::from("Memory is almost full"),
            body: format::fmt(
                "{used} of {total} is in use. Programs may slow down or be closed.",
                &[
                    ("used", &format::size(m.used)),
                    ("total", &format::size(m.total)),
                ],
            ),
            page: "memory",
            past: used >= limits.memory_share,
            back: used < limits.memory_share - MARGIN_SHARE,
            hold: HOLD_MEMORY,
        });
    }
    if let Some(d) = &s.disk {
        for fs in &d.filesystems {
            let used = fs.space.used_fraction();
            let mount = fs.mount_point.display().to_string();
            list.push(Alert {
                id: format!("disk-{mount}"),
                title: format::fmt("{mount} is almost full", &[("mount", &mount)]),
                body: format::fmt(
                    "Only {free} is left.",
                    &[("free", &format::size(fs.space.available))],
                ),
                page: "disk",
                past: used >= limits.disk_share,
                back: used < limits.disk_share - MARGIN_SHARE / 2.0,
                hold: HOLD_DISK,
            });
        }
    }
    if limits.busy_process {
        if let (Some(p), Some(cpu)) = (&s.processes, &s.cpu) {
            let cores = cpu.info.logical.max(1) as f32;
            for info in p.processes.iter() {
                // The share is of the whole machine; one core is 1/cores.
                let of_a_core = info.cpu * cores;
                // A process under half a core is not checked at all; one that
                // had an alert drops out of the list, and it is withdrawn.
                if of_a_core < BUSY_CORE / 2.0 {
                    continue;
                }
                list.push(Alert {
                    id: format!("busy-{}-{}", info.pid, info.start_ticks),
                    title: format::fmt("“{name}” keeps a core busy", &[("name", &info.name)]),
                    body: String::from(
                        "It has used a whole processor core for five minutes. It may be stuck.",
                    ),
                    page: "processes",
                    past: of_a_core >= BUSY_CORE,
                    back: of_a_core < BUSY_CORE / 2.0,
                    hold: HOLD_PROCESS,
                });
            }
        }
    }
    list
}

fn send(app: &gio::Application, alert: &Alert) {
    tracing::info!(id = alert.id, title = alert.title, "alert");
    let notification = gio::Notification::new(&alert.title);
    notification.set_body(Some(&alert.body));
    notification.set_priority(gio::NotificationPriority::High);
    notification
        .set_default_action_and_target_value("app.show-page", Some(&alert.page.to_variant()));
    app.send_notification(Some(&alert.id), &notification);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_alert_waits_is_sent_once_and_goes_with_a_margin() {
        let t0 = Instant::now();
        let at = |s: u64| t0 + Duration::from_secs(s);
        let hold = Duration::from_secs(10);
        let mut t = Trigger::default();
        // A spike shorter than the hold: nothing.
        assert_eq!(t.step(true, false, hold, at(0)), None);
        assert_eq!(t.step(false, false, hold, at(5)), None);
        // Past for the whole hold: sent once.
        assert_eq!(t.step(true, false, hold, at(10)), None);
        assert_eq!(t.step(true, false, hold, at(20)), Some(Change::Send));
        assert_eq!(t.step(true, false, hold, at(30)), None);
        // Just under the limit but not back by the margin: it stays.
        assert_eq!(t.step(false, false, hold, at(31)), None);
        // Back by the margin: withdrawn, and can come again.
        assert_eq!(t.step(false, true, hold, at(32)), Some(Change::Withdraw));
        assert_eq!(t.step(false, true, hold, at(33)), None);
        assert_eq!(t.step(true, false, hold, at(34)), None);
        assert_eq!(t.step(true, false, hold, at(44)), Some(Change::Send));
    }

    #[test]
    fn limits_decide_which_conditions_are_past() {
        use corewatch_core::disk::{DiskSample, Filesystem, Space};
        let mut snapshot = Snapshot::new();
        let fs = |mount: &str, used: u64, available: u64| Filesystem {
            mount_point: mount.into(),
            source: "/dev/sda1".into(),
            fs_type: "ext4".into(),
            space: Space {
                total: used + available,
                used,
                available,
            },
        };
        snapshot.disk = Some(DiskSample {
            filesystems: vec![fs("/", 97, 3), fs("/home", 50, 50)],
            ..DiskSample::default()
        });
        let limits = AlertSettings::default();
        let found = alerts(&snapshot, &limits);
        let past: Vec<&str> = found
            .iter()
            .filter(|a| a.past)
            .map(|a| a.id.as_str())
            .collect();
        assert_eq!(past, ["disk-/"]);
        assert_eq!(found[0].title, "/ is almost full");
        assert_eq!(found[0].page, "disk");
    }

    #[test]
    fn no_hold_sends_at_once() {
        let mut t = Trigger::default();
        assert_eq!(
            t.step(true, false, Duration::ZERO, Instant::now()),
            Some(Change::Send)
        );
    }
}
