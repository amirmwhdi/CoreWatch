//! `SensorGroup`: the "Sensors" group at the end of a resource page.
//!
//! Each page shows the hwmon sensors that belong to it ([`Home`]): the CPU
//! page its package temperature, the average of its cores and the CPU fan, the Disks page the
//! drive temperatures. What is left stays on the Sensors page. The group is
//! hidden while the page has no sensor.

use crate::i18n::{self, gettext};
use adw::prelude::*;
use corewatch_core::sensors::{Chip, Home, SensorSample, Temperature};
use std::cell::RefCell;

/// One row: its title and subtitle. The rows are rebuilt only when these or
/// the units change.
type Shape = (Vec<(String, String)>, u32);

pub struct SensorGroup {
    group: adw::PreferencesGroup,
    home: Home,
    shape: RefCell<Shape>,
    rows: RefCell<Vec<adw::ActionRow>>,
    values: RefCell<Vec<gtk::Label>>,
}

/// One sensor reading of a chip, with what it is called on screen.
pub struct Reading<'a> {
    pub chip: &'a Chip,
    pub title: String,
    /// "Critical at 95 °C", when the chip reports a critical point.
    pub critical: Option<String>,
    pub value: String,
}

impl Reading<'_> {
    /// The device and the critical point, for a row outside its chip's group.
    pub fn subtitle(&self) -> String {
        let device = self.chip.model.as_deref().unwrap_or(&self.chip.driver);
        match &self.critical {
            Some(critical) => i18n::fmt(
                // Translators: e.g. "k10temp · Critical at 95 °C"
                &gettext("{device} · {critical}"),
                &[("device", &device), ("critical", critical)],
            ),
            None => device.to_owned(),
        }
    }
}

impl SensorGroup {
    pub fn new(group: adw::PreferencesGroup, home: Home) -> Self {
        Self {
            group,
            home,
            shape: RefCell::default(),
            rows: RefCell::default(),
            values: RefCell::default(),
        }
    }

    pub fn update(&self, sensors: Option<&SensorSample>) {
        let readings = sensors.map(|s| readings(s, self.home)).unwrap_or_default();
        let shape: Shape = (
            readings
                .iter()
                .map(|r| (r.title.clone(), r.subtitle()))
                .collect(),
            i18n::units_generation(),
        );
        if *self.shape.borrow() != shape {
            for row in self.rows.take() {
                self.group.remove(&row);
            }
            let mut rows = Vec::with_capacity(readings.len());
            let mut values = Vec::with_capacity(readings.len());
            for r in &readings {
                let row = adw::ActionRow::builder()
                    .title(&r.title)
                    .subtitle(r.subtitle())
                    .build();
                values.push(value_label(&row));
                self.group.add(&row);
                rows.push(row);
            }
            self.group.set_visible(!rows.is_empty());
            *self.rows.borrow_mut() = rows;
            *self.values.borrow_mut() = values;
            *self.shape.borrow_mut() = shape;
        }
        for (label, r) in self.values.borrow().iter().zip(&readings) {
            if label.label() != r.value {
                label.set_label(&r.value);
            }
        }
    }
}

/// Every reading that belongs on `home`, temperatures before fans per chip.
pub fn readings(sensors: &SensorSample, home: Home) -> Vec<Reading<'_>> {
    let mut out = Vec::new();
    for chip in &sensors.chips {
        // Intel's coretemp has one sensor per core; they are shown as one
        // average, after the package temperature.
        let mut cores: Vec<&Temperature> = Vec::new();
        for (i, t) in chip.temperatures.iter().enumerate() {
            if chip.home(t.label.as_deref()) != home || chip.shown_by_page(t.index) {
                continue;
            }
            if chip.driver == "coretemp" && t.label.as_deref().is_some_and(is_core_label) {
                cores.push(t);
                continue;
            }
            let title = t
                .label
                .clone()
                .unwrap_or_else(|| numbered(&gettext("Temperature"), i));
            let critical = t.critical.map(critical_at);
            out.push(Reading {
                chip,
                title,
                critical,
                value: i18n::celsius(t.celsius),
            });
        }
        if !cores.is_empty() {
            let average = cores.iter().map(|t| t.celsius).sum::<f32>() / cores.len() as f32;
            out.push(Reading {
                chip,
                title: gettext("Core Average"),
                critical: cores.iter().find_map(|t| t.critical).map(critical_at),
                value: i18n::celsius(average),
            });
        }
        for (i, f) in chip.fans.iter().enumerate() {
            if chip.home(f.label.as_deref()) != home || chip.fan_shown_by_page(f.index) {
                continue;
            }
            let title = f
                .label
                .clone()
                .unwrap_or_else(|| numbered(&gettext("Fan"), i));
            out.push(Reading {
                chip,
                title,
                critical: None,
                value: rpm(f.rpm),
            });
        }
    }
    out
}

fn critical_at(celsius: f32) -> String {
    i18n::fmt(
        &gettext("Critical at {value}"),
        &[("value", &i18n::celsius(celsius))],
    )
}

/// "Core 3", one core's sensor on coretemp.
fn is_core_label(label: &str) -> bool {
    label
        .strip_prefix("Core ")
        .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

pub fn value_label(row: &adw::ActionRow) -> gtk::Label {
    let label = gtk::Label::builder()
        .label("…")
        .valign(gtk::Align::Center)
        .css_classes(["numeric"])
        .build();
    row.add_suffix(&label);
    label
}

/// "Fan 2" for the second unlabeled fan.
pub fn numbered(what: &str, index: usize) -> String {
    i18n::fmt(
        // Translators: an unnamed sensor and its number, e.g. "Fan 2"
        &gettext("{what} {number}"),
        &[("what", &what), ("number", &(index + 1))],
    )
}

pub fn rpm(value: u32) -> String {
    // Translators: fan speed in revolutions per minute, e.g. "1200 RPM"
    i18n::fmt(&gettext("{value} RPM"), &[("value", &value)])
}

#[cfg(test)]
mod tests {
    use super::*;
    use corewatch_core::sensors::ChipKind;

    #[test]
    fn intel_cores_are_shown_as_one_average() {
        let t = |label: &str, index, celsius| Temperature {
            label: Some(label.into()),
            index,
            celsius,
            critical: Some(100.0),
        };
        let sample = SensorSample {
            chips: vec![Chip {
                driver: "coretemp".into(),
                kind: ChipKind::Cpu,
                model: None,
                temperatures: vec![
                    t("Package id 0", 1, 62.0),
                    t("Core 0", 2, 50.0),
                    t("Core 4", 6, 60.0),
                ],
                fans: vec![],
            }],
        };
        let readings = readings(&sample, Home::Cpu);
        let titles: Vec<&str> = readings.iter().map(|r| r.title.as_str()).collect();
        assert_eq!(titles, ["Package id 0", "Core Average"]);
        assert!(readings[1].value.starts_with("55"), "{}", readings[1].value);
        assert!(is_core_label("Core 12"));
        assert!(!is_core_label("Core"));
        assert!(!is_core_label("Package id 0"));
    }
}
