//! The battery's charge over the last day, from UPower.
//!
//! UPower records each battery's charge level as it changes and keeps that
//! history across restarts of Corewatch; asking it gives the last day at
//! once, where Corewatch's own graphs only know what happened while it ran.

use gtk::{gio, glib};
use std::time::Duration;

/// How far back the day graph reaches.
pub const SPAN: Duration = Duration::from_secs(24 * 3600);
/// Points of the day graph: one per 15 minutes.
pub const POINTS: usize = 96;
/// How often the history is asked for again.
pub const REFRESH: Duration = Duration::from_secs(5 * 60);

/// The charge (0.0..=1.0) of battery `name` (the kernel's `BAT0`) at the end
/// of each of the last [`POINTS`] stretches of [`SPAN`], oldest first,
/// starting with the first stretch UPower knows about. Empty when UPower is
/// missing or has no history.
pub async fn charge(name: &str) -> Vec<f32> {
    let Ok(bus) = gio::bus_get_future(gio::BusType::System).await else {
        return Vec::new();
    };
    let path = format!("/org/freedesktop/UPower/devices/battery_{name}");
    let reply = bus
        .call_future(
            Some("org.freedesktop.UPower"),
            &path,
            "org.freedesktop.UPower.Device",
            "GetHistory",
            Some(&("charge", SPAN.as_secs() as u32, POINTS as u32 * 2).into()),
            Some(glib::VariantTy::new("(a(udu))").expect("valid type")),
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await;
    let samples: Vec<(u32, f64, u32)> = match reply {
        Ok(reply) => reply.child_value(0).get().unwrap_or_default(),
        Err(e) => {
            tracing::debug!("no battery history from UPower: {e}");
            return Vec::new();
        }
    };
    let now = glib::real_time() / 1_000_000;
    resample(&samples, u64::try_from(now).unwrap_or(0))
}

/// The charge at the end of each 15-minute stretch up to `now` (seconds),
/// from `(time, percent, state)` samples in any order: the last sample at or
/// before the stretch's end. Stretches before the first sample are left out.
pub fn resample(samples: &[(u32, f64, u32)], now: u64) -> Vec<f32> {
    let mut sorted: Vec<(u64, f32)> = samples
        .iter()
        .filter(|(t, v, _)| *t > 0 && (0.0..=100.0).contains(v))
        .map(|(t, v, _)| (u64::from(*t), *v as f32 / 100.0))
        .collect();
    sorted.sort_by_key(|(t, _)| *t);
    let stretch = SPAN.as_secs() / POINTS as u64;
    let start = now.saturating_sub(SPAN.as_secs());
    (1..=POINTS as u64)
        .filter_map(|i| {
            let end = start + i * stretch;
            sorted
                .iter()
                .rev()
                .find(|(t, _)| *t <= end)
                .map(|(_, v)| *v)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_become_quarter_hours() {
        let now = 1_000_000;
        let day = SPAN.as_secs() as u32;
        let samples = [
            // Unordered, as UPower may send them.
            (now as u32 - 600, 80.0, 2),
            (now as u32 - day / 2, 100.0, 4),
            (now as u32 - day / 4, 90.0, 2),
            (0, 50.0, 0), // no time: left out
        ];
        let points = resample(&samples, now);
        // From half a day ago: the stretch that ends right at that sample,
        // and the 48 after it.
        assert_eq!(points.len(), 49);
        assert_eq!(points[0], 1.0);
        assert_eq!(points[points.len() - 1], 0.8);
        assert!(points.contains(&0.9));
        assert!(resample(&[], now).is_empty());
    }
}
