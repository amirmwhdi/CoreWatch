//! The Health row of a disk block: a summary with the health percentage,
//! which opens to every SMART figure the drive gives.

use crate::drive_smart::{Health, Protocol};
use crate::format;
use adw::prelude::*;
use gtk::glib;
use std::cell::RefCell;

/// An expander row: "Health", the status as its subtitle, the percentage
/// and a level bar at its end, and one row per figure inside.
pub struct HealthRow {
    row: adw::ExpanderRow,
    percent: gtk::Label,
    bar: gtk::LevelBar,
    /// Detail rows, rebuilt only when the set of titles changes.
    details: RefCell<Vec<(&'static str, adw::ActionRow)>>,
}

impl HealthRow {
    pub fn new() -> Self {
        let row = adw::ExpanderRow::builder()
            .title("Health")
            .subtitle("…")
            .visible(false)
            .build();
        let percent = gtk::Label::builder()
            .valign(gtk::Align::Center)
            .css_classes(["numeric", "heading"])
            .build();
        let bar = gtk::LevelBar::builder()
            .width_request(120)
            .valign(gtk::Align::Center)
            .build();
        // Warning colour below half, normal above.
        for name in [
            gtk::LEVEL_BAR_OFFSET_LOW,
            gtk::LEVEL_BAR_OFFSET_HIGH,
            gtk::LEVEL_BAR_OFFSET_FULL,
        ] {
            bar.remove_offset_value(Some(name));
        }
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 0.5);
        bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, 1.0);
        row.add_suffix(&percent);
        row.add_suffix(&bar);
        Self {
            row,
            percent,
            bar,
            details: RefCell::default(),
        }
    }

    pub fn widget(&self) -> &adw::ExpanderRow {
        &self.row
    }

    /// Show `health`, or hide the row when there is none.
    pub fn update(&self, health: Option<&Health>) {
        self.row.set_visible(health.is_some());
        let Some(health) = health else { return };

        let text = summary(health);
        if self.row.subtitle() != text {
            self.row.set_subtitle(&text);
        }
        let level = level(health);
        for class in ["error", "warning"] {
            if Some(class) == level {
                self.row.add_css_class(class);
            } else {
                self.row.remove_css_class(class);
            }
        }

        self.percent.set_visible(health.percent.is_some());
        self.bar.set_visible(health.percent.is_some());
        if let Some(p) = health.percent {
            let text = percent_text(health).unwrap_or_default();
            if self.percent.label() != text {
                self.percent.set_label(&text);
            }
            self.bar.set_value(f64::from(p) / 100.0);
            self.bar
                .update_property(&[gtk::accessible::Property::Label(&text)]);
        }

        let details = details(health);
        let mut rows = self.details.borrow_mut();
        let same =
            rows.len() == details.len() && rows.iter().zip(&details).all(|((a, _), (b, _))| a == b);
        if !same {
            for (_, row) in rows.drain(..) {
                self.row.remove(&row);
            }
            for (title, _) in &details {
                let row = adw::ActionRow::builder()
                    .title(*title)
                    .subtitle_selectable(true)
                    .css_classes(["property"])
                    .build();
                self.row.add_row(&row);
                rows.push((title, row));
            }
        }
        for ((_, row), (_, value)) in rows.iter().zip(&details) {
            if row.subtitle().as_deref() != Some(value.as_str()) {
                row.set_subtitle(value);
            }
        }
    }
}

/// `"error"` when the drive predicts its own failure, `"warning"` for any
/// other problem.
pub fn level(health: &Health) -> Option<&'static str> {
    if health.failing {
        Some("error")
    } else if !health.is_good() {
        Some("warning")
    } else {
        None
    }
}

/// "97%", or "~92%" for an estimate.
fn percent_text(health: &Health) -> Option<String> {
    let p = health.percent?;
    let text = format::percent(f32::from(p) / 100.0);
    Some(if health.estimated {
        format!("~{text}")
    } else {
        text
    })
}

/// "Good", "Warning · 8 bad sectors · last self-test failed",
/// "Failing · back up its data now".
pub fn summary(health: &Health) -> String {
    let mut parts: Vec<String> = Vec::new();
    if health.failing {
        parts.push("Failing · back up its data now".to_owned());
    } else if health.is_good() {
        parts.push("Good".to_owned());
    } else {
        parts.push("Warning".to_owned());
        parts.extend(health.critical_warnings.iter().map(|w| nvme_warning(w)));
        if let Some(n) = health.attributes_failing.filter(|n| *n > 0) {
            parts.push(format::fmt(
                "{n} {attributes} past threshold",
                &[
                    ("n", &n),
                    ("attributes", &format::plural(n, "attribute", "attributes")),
                ],
            ));
        }
        if let Some(n) = health.media_errors.filter(|n| *n > 0) {
            parts.push(format::fmt(
                "{n} media {errors}",
                &[("n", &n), ("errors", &format::plural(n, "error", "errors"))],
            ));
        }
    }
    // Bad sectors matter even on a failing drive.
    if let Some(n) = health.bad_sectors.filter(|n| *n > 0) {
        parts.push(format::fmt(
            "{n} bad {sectors}",
            &[
                ("n", &n),
                ("sectors", &format::plural(n, "sector", "sectors")),
            ],
        ));
    }
    if health.selftest_failed {
        parts.push("last self-test failed".to_owned());
    }
    parts.join(" · ")
}

/// Every figure the drive gives, as (title, value), in a fixed order.
pub fn details(health: &Health) -> Vec<(&'static str, String)> {
    let mut out: Vec<(&'static str, String)> = Vec::new();
    if let Some(text) = percent_text(health) {
        if health.estimated {
            out.push((
                "Estimated Health",
                format::fmt(
                    "{p} · from bad sectors and the drive's reliability attributes; hard disks report no life left",
                    &[("p", &text)],
                ),
            ));
        } else {
            out.push(("Life Left", text));
        }
    }
    if let Some(spare) = health.available_spare {
        let mut text = format::percent(f32::from(spare) / 100.0);
        if let Some(t) = health.spare_threshold {
            text = format::fmt(
                "{spare} · warns below {threshold}",
                &[
                    ("spare", &text),
                    ("threshold", &format::percent(f32::from(t) / 100.0)),
                ],
            );
        }
        out.push(("Spare Capacity", text));
    }
    if let Some(hours) = health.power_on_hours {
        out.push(("Powered On", power_on(hours)));
    }
    let mut count = |title, n: Option<u64>| {
        if let Some(n) = n {
            out.push((title, thousands(n)));
        }
    };
    count("Power Cycles", health.power_cycles);
    count("Unsafe Shutdowns", health.unsafe_shutdowns);
    if let Some(bytes) = health.data_written {
        out.push(("Data Written", format::size(bytes)));
    }
    if let Some(bytes) = health.data_read {
        out.push(("Data Read", format::size(bytes)));
    }
    let mut count = |title, n: Option<u64>| {
        if let Some(n) = n {
            out.push((title, thousands(n)));
        }
    };
    count("Reallocated Sectors", health.reallocated_sectors);
    count("Pending Sectors", health.pending_sectors);
    count("Uncorrectable Sectors", health.uncorrectable_sectors);
    // UDisks' total when the attributes did not come.
    if health.reallocated_sectors.is_none() && health.pending_sectors.is_none() {
        count("Bad Sectors", health.bad_sectors);
    }
    count("Media Errors", health.media_errors);
    count("Error Log Entries", health.error_log_entries);
    if let Some(now) = health.attributes_failing {
        let text = match health.attributes_failed_in_past {
            Some(past) => format::fmt(
                "{now} now · {past} in the past",
                &[("now", &now), ("past", &past)],
            ),
            None => now.to_string(),
        };
        out.push(("Attributes Past Threshold", text));
    }
    if let Some(minutes) = health.minutes_too_hot {
        out.push((
            "Time Too Hot",
            format::fmt(
                "{n} {minutes} above the warning temperature",
                &[
                    ("n", &thousands(minutes)),
                    ("minutes", &format::plural(minutes, "minute", "minutes")),
                ],
            ),
        ));
    }
    if !health.critical_warnings.is_empty() {
        let warnings: Vec<String> = health
            .critical_warnings
            .iter()
            .map(|w| nvme_warning(w))
            .collect();
        out.push(("Critical Warnings", warnings.join(", ")));
    }
    if let Some(status) = &health.selftest {
        out.push(("Last Self-Test", selftest(status)));
    }
    out.push((
        "Interface",
        match (health.protocol, health.rotational) {
            (Protocol::Nvme, _) => "NVMe SMART",
            (Protocol::Ata, Some(true)) => "ATA SMART · hard disk",
            (Protocol::Ata, Some(false)) => "ATA SMART · SSD",
            (Protocol::Ata, None) => "ATA SMART",
        }
        .to_owned(),
    ));
    if let Some(ago) = checked(health.updated) {
        out.push(("Read by UDisks", ago));
    }
    out
}

/// "36 hours", "93 days · 2,248 hours", "2 years 40 days · 18,500 hours".
fn power_on(hours: u64) -> String {
    let in_hours = format::fmt(
        "{n} {hours}",
        &[
            ("n", &thousands(hours)),
            ("hours", &format::plural(hours, "hour", "hours")),
        ],
    );
    if hours < 48 {
        return in_hours;
    }
    let days = hours / 24;
    let (years, days) = (days / 365, days % 365);
    let span = if years == 0 {
        format::fmt("{d} days", &[("d", &days)])
    } else {
        format::fmt(
            "{y} {years} {d} {days}",
            &[
                ("y", &years),
                ("years", &format::plural(years, "year", "years")),
                ("d", &days),
                ("days", &format::plural(days, "day", "days")),
            ],
        )
    };
    format::fmt("{span} · {hours}", &[("span", &span), ("hours", &in_hours)])
}

/// "12,345".
fn thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "12 minutes ago"; `None` when the time is unknown.
fn checked(updated: u64) -> Option<String> {
    let now = glib::real_time() / 1_000_000;
    let ago = u64::try_from(now).ok()?.checked_sub(updated)? / 60;
    Some(match ago {
        0 => "Just now".to_owned(),
        n if n < 120 => format::fmt(
            "{n} {minutes} ago",
            &[
                ("n", &n),
                ("minutes", &format::plural(n, "minute", "minutes")),
            ],
        ),
        n => format::fmt(
            "{n} {hours} ago",
            &[
                ("n", &(n / 60)),
                ("hours", &format::plural(n / 60, "hour", "hours")),
            ],
        ),
    })
}

/// UDisks' name of a self-test result, in words.
fn selftest(status: &str) -> String {
    match status {
        "success" => "Passed",
        "inprogress" => "Running",
        "aborted" => "Aborted",
        "interrupted" => "Interrupted",
        "error_read" => "Failed · read error",
        "error_electrical" => "Failed · electrical error",
        "error_servo" => "Failed · servo error",
        "error_handling" => "Failed · handling damage",
        "known_seg_fail" | "unknown_seg_fail" => "Failed · segment error",
        s if s.starts_with("error") || s.starts_with("fatal") => "Failed",
        "ctrl_reset" => "Aborted by a controller reset",
        "ns_removed" => "Aborted · namespace removed",
        other => return other.replace('_', " "),
    }
    .to_owned()
}

/// UDisks' name of an NVMe critical warning, in words.
fn nvme_warning(warning: &str) -> String {
    match warning {
        "spare" => "spare capacity low",
        "temperature" => "temperature out of range",
        "degraded" => "reliability degraded",
        "readonly" => "read-only",
        "volatile_mem" => "backup memory failed",
        "pmr_readonly" => "persistent memory read-only",
        other => other,
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summaries() {
        let good = Health {
            power_on_hours: Some(9000),
            bad_sectors: Some(0),
            attributes_failing: Some(0),
            ..Health::default()
        };
        assert_eq!(summary(&good), "Good");
        assert_eq!(level(&good), None);

        let worn = Health {
            bad_sectors: Some(1),
            selftest_failed: true,
            ..Health::default()
        };
        assert_eq!(
            summary(&worn),
            "Warning · 1 bad sector · last self-test failed"
        );
        assert_eq!(level(&worn), Some("warning"));

        let failing = Health {
            failing: true,
            bad_sectors: Some(120),
            ..Health::default()
        };
        assert_eq!(
            summary(&failing),
            "Failing · back up its data now · 120 bad sectors"
        );
        assert_eq!(level(&failing), Some("error"));

        let nvme = Health {
            critical_warnings: vec!["spare".to_owned()],
            ..Health::default()
        };
        assert_eq!(summary(&nvme), "Warning · spare capacity low");
    }

    #[test]
    fn hard_disk_details() {
        let disk = Health {
            rotational: Some(true),
            percent: Some(92),
            estimated: true,
            power_on_hours: Some(18_500),
            power_cycles: Some(1_204),
            reallocated_sectors: Some(8),
            pending_sectors: Some(0),
            attributes_failing: Some(0),
            attributes_failed_in_past: Some(1),
            selftest: Some("error_read".to_owned()),
            ..Health::default()
        };
        let details = details(&disk);
        let get = |t: &str| {
            details
                .iter()
                .find(|(k, _)| *k == t)
                .map(|(_, v)| v.as_str())
        };
        assert!(get("Estimated Health").unwrap().starts_with("~92"));
        assert_eq!(get("Life Left"), None);
        assert_eq!(get("Powered On"), Some("2 years 40 days · 18,500 hours"));
        assert_eq!(get("Power Cycles"), Some("1,204"));
        assert_eq!(get("Reallocated Sectors"), Some("8"));
        assert_eq!(get("Bad Sectors"), None);
        assert_eq!(
            get("Attributes Past Threshold"),
            Some("0 now · 1 in the past")
        );
        assert_eq!(get("Last Self-Test"), Some("Failed · read error"));
        assert_eq!(get("Interface"), Some("ATA SMART · hard disk"));
    }

    #[test]
    fn nvme_details() {
        let nvme = Health {
            protocol: Protocol::Nvme,
            percent: Some(97),
            available_spare: Some(100),
            spare_threshold: Some(10),
            power_on_hours: Some(2248),
            selftest: Some("success".to_owned()),
            ..Health::default()
        };
        let details = details(&nvme);
        let get = |t: &str| {
            details
                .iter()
                .find(|(k, _)| *k == t)
                .map(|(_, v)| v.as_str())
        };
        assert!(get("Life Left").unwrap().starts_with("97"));
        assert!(get("Spare Capacity").unwrap().contains("warns below 10"));
        assert_eq!(get("Powered On"), Some("93 days · 2,248 hours"));
        assert_eq!(get("Last Self-Test"), Some("Passed"));
        assert_eq!(get("Interface"), Some("NVMe SMART"));
    }

    #[test]
    fn numbers() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(1000), "1,000");
        assert_eq!(thousands(1_234_567), "1,234,567");
        assert_eq!(power_on(30), "30 hours");
        assert_eq!(power_on(1), "1 hour");
        assert_eq!(power_on(24 * 365), "1 year 0 days · 8,760 hours");
    }
}
