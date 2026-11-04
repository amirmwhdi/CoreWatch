//! Text for the UI: named templates, plurals, and numbers with units.
//!
//! Corewatch is in English only. Log messages and `--help` are English too.

use gtk::glib;
use std::cell::Cell;
use std::fmt::Display;

/// Fill named placeholders in a template:
/// `fmt("{used} of {total}", &[("used", &a), ("total", &b)])`.
///
/// Unknown or unclosed placeholders are left as they are, and a value is
/// never scanned for placeholders itself.
pub fn fmt(template: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(template.len() + 16);
    let mut rest = template;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after.find('}').and_then(|end| {
            args.iter()
                .find(|(name, _)| *name == &after[..end])
                .map(|(_, v)| (end, v))
        });
        match value {
            Some((end, value)) => {
                out.push_str(&value.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// `one` for a count of 1, `many` otherwise.
pub fn plural<'a>(n: impl TryInto<u64>, one: &'a str, many: &'a str) -> &'a str {
    if n.try_into().ok() == Some(1) {
        one
    } else {
        many
    }
}

/// How long tasks waited for a resource, for a "Pressure" row:
/// "Some tasks waited 12% of the time, all tasks 2% (last 10 s)".
pub fn pressure(p: &corewatch_core::pressure::Pressure, with_full: bool) -> String {
    match p.full.filter(|_| with_full) {
        Some(full) => fmt(
            "Some tasks waited {some} of the time, all tasks {full} (last 10 s)",
            &[("some", &percent(p.some)), ("full", &percent(full))],
        ),
        None => fmt(
            "Some tasks waited {some} of the time (last 10 s)",
            &[("some", &percent(p.some))],
        ),
    }
}

/// Show a "Pressure" row with `p`, or hide it when there is none.
pub fn set_pressure_row(
    row: &adw::ActionRow,
    p: Option<&corewatch_core::pressure::Pressure>,
    with_full: bool,
) {
    use adw::prelude::*;
    row.set_visible(p.is_some());
    if let Some(p) = p {
        row.set_subtitle(&pressure(p, with_full));
    }
}

/// A share 0.0..=1.0 as a whole percentage, e.g. "37%".
pub fn percent(fraction: f32) -> String {
    format!("{:.0}%", fraction * 100.0)
}

/// How numbers with units are shown; chosen in Preferences.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Units {
    pub temperature: TemperatureUnit,
    pub sizes: SizeUnits,
    pub network: NetworkUnits,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum TemperatureUnit {
    #[default]
    Celsius,
    Fahrenheit,
}

/// Binary: 1 KiB = 1024 bytes. Decimal: 1 kB = 1000 bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SizeUnits {
    #[default]
    Binary,
    Decimal,
}

/// Network speeds in bytes (MB/s, like files) or bits (Mb/s, like
/// internet plans).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkUnits {
    #[default]
    Bytes,
    Bits,
}

thread_local! {
    /// The UI thread's current units; every formatter below reads them, so a
    /// change shows on the next tick without rebuilding any page.
    static UNITS: Cell<(Units, u32)> = const { Cell::new((
        Units {
            temperature: TemperatureUnit::Celsius,
            sizes: SizeUnits::Binary,
            network: NetworkUnits::Bytes,
        },
        0,
    )) };
}

pub fn units() -> Units {
    UNITS.with(|u| u.get().0)
}

pub fn set_units(units: Units) {
    UNITS.with(|u| {
        let (old, generation) = u.get();
        if old != units {
            u.set((units, generation.wrapping_add(1)));
        }
    });
}

/// Changes every time the units change. Pages that format a label only once
/// (a disk's capacity) compare it to know when to format again.
pub fn units_generation() -> u32 {
    UNITS.with(|u| u.get().1)
}

/// A size, e.g. "4.1 GiB" or "4.4 GB".
pub fn size(bytes: u64) -> String {
    let flags = match units().sizes {
        SizeUnits::Binary => glib::FormatSizeFlags::IEC_UNITS,
        SizeUnits::Decimal => glib::FormatSizeFlags::DEFAULT,
    };
    glib::format_size_full(bytes, flags).into()
}

/// A transfer speed of a disk or file, e.g. "12.3 MiB/s".
pub fn speed(bytes_per_sec: u64) -> String {
    format!("{}/s", size(bytes_per_sec))
}

/// A network speed: like [`speed`], or in bits, e.g. "98.4 Mb/s".
pub fn net_speed(bytes_per_sec: u64) -> String {
    match units().network {
        NetworkUnits::Bytes => speed(bytes_per_sec),
        NetworkUnits::Bits => {
            let bits = glib::format_size_full(
                bytes_per_sec.saturating_mul(8),
                glib::FormatSizeFlags::BITS,
            );
            format!("{bits}/s")
        }
    }
}

/// A temperature given in degrees Celsius, in the chosen unit, e.g. "54 °C"
/// or "129 °F".
pub fn celsius(value: f32) -> String {
    match units().temperature {
        TemperatureUnit::Celsius => format!("{value:.0} °C"),
        TemperatureUnit::Fahrenheit => format!("{:.0} °F", value * 9.0 / 5.0 + 32.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fills_named_placeholders_in_any_order() {
        let text = fmt(
            "{total} total, {used} used",
            &[("used", &1), ("total", &"2 GB")],
        );
        assert_eq!(text, "2 GB total, 1 used");
    }

    #[test]
    fn repeats_and_unknown_names_are_handled() {
        assert_eq!(fmt("{a}-{a}", &[("a", &7)]), "7-7");
        assert_eq!(fmt("{b} {a}", &[("a", &1)]), "{b} 1");
        assert_eq!(fmt("open { brace", &[]), "open { brace");
    }

    #[test]
    fn values_are_not_expanded_again() {
        assert_eq!(fmt("{a}", &[("a", &"{a}")]), "{a}");
    }

    #[test]
    fn english_plurals() {
        assert_eq!(plural(1, "process", "processes"), "process");
        assert_eq!(plural(0, "process", "processes"), "processes");
        assert_eq!(plural(2, "process", "processes"), "processes");
    }

    #[test]
    fn non_ascii_text_around_placeholders() {
        assert_eq!(fmt("— {n} —", &[("n", &3)]), "— 3 —");
    }
}
