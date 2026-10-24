//! Battery and power supply module: capacity, charging state, health, rate, and time remaining.

use crate::{CollectError, Collector, Snapshot, SysRoot};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BatteryState {
    Charging,
    Discharging,
    NotCharging,
    Full,
    Unknown,
}

impl BatteryState {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Charging => "Charging",
            Self::Discharging => "Discharging",
            Self::NotCharging => "Not Charging",
            Self::Full => "Full",
            Self::Unknown => "Unknown",
        }
    }
}

impl std::str::FromStr for BatteryState {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.trim().to_lowercase().as_str() {
            "charging" => Self::Charging,
            "discharging" => Self::Discharging,
            "not charging" => Self::NotCharging,
            "full" => Self::Full,
            _ => Self::Unknown,
        })
    }
}

/// One physical battery.
#[derive(Debug, Clone, PartialEq)]
pub struct BatteryDevice {
    pub name: String,
    pub model: String,
    pub technology: String,
    pub state: BatteryState,
    /// Percentage in 0.0..=1.0.
    pub capacity: f32,
    /// Current energy in Watt-hours (Wh).
    pub energy_wh: Option<f64>,
    /// Full charge energy in Watt-hours (Wh).
    pub energy_full_wh: Option<f64>,
    /// Design capacity energy in Watt-hours (Wh).
    pub energy_design_wh: Option<f64>,
    /// Battery health: energy_full / energy_design in 0.0..=1.0.
    pub health: Option<f32>,
    /// Instantaneous charge or discharge rate in Watts (W).
    pub power_w: Option<f64>,
    /// Voltage in Volts (V).
    pub voltage_v: Option<f64>,
    /// Estimated time to empty (if discharging) or full (if charging) in seconds.
    pub time_remaining_secs: Option<u64>,
    /// Charge cycles.
    pub cycle_count: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct BatterySample {
    pub ac_online: Option<bool>,
    pub batteries: Vec<BatteryDevice>,
}

impl BatterySample {
    pub fn primary_capacity(&self) -> Option<f32> {
        self.batteries.first().map(|b| b.capacity)
    }

    pub fn primary_state(&self) -> Option<BatteryState> {
        self.batteries.first().map(|b| b.state)
    }
}

/// Battery and AC files sit behind ACPI methods on most laptops, so one read
/// can cost milliseconds. The charge changes slowly: refresh at most this often.
pub const REFRESH: Duration = Duration::from_secs(5);

/// Facts that do not change while the battery is plugged in, read once.
struct BatteryDetector {
    name: String,
    path: PathBuf,
    model: String,
    technology: String,
}

pub struct BatteryCollector {
    detectors: Vec<BatteryDetector>,
    ac_paths: Vec<PathBuf>,
    last: Option<(Instant, BatterySample)>,
}

impl BatteryCollector {
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        let power_supply_dir = root.sys_path("class/power_supply");
        let mut detectors = Vec::new();
        let mut ac_paths = Vec::new();

        if let Ok(entries) = fs::read_dir(&power_supply_dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                let name = entry.file_name().to_string_lossy().to_string();
                let type_str = read_trimmed(&path, "type").unwrap_or_default();

                // Mice, keyboards and headsets report `scope = Device`: they
                // are not the battery that powers this machine.
                if read_trimmed(&path, "scope").as_deref() == Some("Device") {
                    continue;
                }
                if type_str.eq_ignore_ascii_case("battery") || name.starts_with("BAT") {
                    let model = read_trimmed(&path, "model_name")
                        .or_else(|| read_trimmed(&path, "manufacturer"))
                        .unwrap_or_else(|| name.clone());
                    let technology =
                        read_trimmed(&path, "technology").unwrap_or_else(|| "Li-ion".to_string());
                    detectors.push(BatteryDetector {
                        name,
                        path,
                        model,
                        technology,
                    });
                } else if type_str.eq_ignore_ascii_case("mains")
                    || name.starts_with("AC")
                    || name.starts_with("ADP")
                {
                    ac_paths.push(path.join("online"));
                }
            }
        }

        if detectors.is_empty() {
            return Err(CollectError::parse(
                "sys/class/power_supply",
                "no battery detected",
            ));
        }

        detectors.sort_by(|a, b| a.name.cmp(&b.name));
        ac_paths.sort();
        Ok(Self {
            detectors,
            ac_paths,
            last: None,
        })
    }

    /// [`Collector::collect`] with an explicit clock; used by tests.
    pub fn collect_at(&mut self, out: &mut Snapshot, now: Instant) {
        let ac_online = self.ac_paths.iter().find_map(|p| {
            fs::read_to_string(p)
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
                .map(|val| val == 1)
        });

        // Reuse the last reading unless it is old or the charger was just
        // plugged in or out, so the state never lags behind the AC row.
        if let Some((at, sample)) = &self.last {
            if sample.ac_online == ac_online && now.saturating_duration_since(*at) < REFRESH {
                out.battery = Some(sample.clone());
                return;
            }
        }

        let sample = BatterySample {
            ac_online,
            batteries: self.detectors.iter().map(read_battery).collect(),
        };
        out.battery = Some(sample.clone());
        self.last = Some((now, sample));
    }
}

impl Collector for BatteryCollector {
    fn name(&self) -> &'static str {
        "battery"
    }

    fn collect(&mut self, _root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        self.collect_at(out, Instant::now());
        Ok(())
    }
}

fn read_trimmed(dir: &Path, file: &str) -> Option<String> {
    fs::read_to_string(dir.join(file))
        .ok()
        .map(|s| s.trim().to_string())
}

fn read_battery(det: &BatteryDetector) -> BatteryDevice {
    let read_u64 = |file: &str| -> Option<u64> { read_trimmed(&det.path, file)?.parse().ok() };
    // Some drivers report the discharge current or power as a negative number.
    let read_abs = |file: &str| -> Option<u64> {
        read_trimmed(&det.path, file)?
            .parse::<i64>()
            .ok()
            .map(i64::unsigned_abs)
    };

    let state = read_trimmed(&det.path, "status")
        .and_then(|s| s.parse().ok())
        .unwrap_or(BatteryState::Unknown);

    let capacity_val = read_u64("capacity").unwrap_or(0);
    let capacity = (capacity_val as f32 / 100.0).clamp(0.0, 1.0);
    let cycle_count = read_u64("cycle_count")
        .and_then(|c| u32::try_from(c).ok())
        .filter(|c| *c > 0);

    let voltage_v = read_u64("voltage_now").map(|uv| uv as f64 / 1_000_000.0);

    // Energy in µWh, or charge in µAh times the voltage.
    let energy = |energy_file: &str, charge_file: &str| -> Option<f64> {
        let uwh = read_u64(energy_file)
            .map(|e| e as f64)
            .or_else(|| Some(read_u64(charge_file)? as f64 * voltage_v?))?;
        Some(uwh / 1_000_000.0)
    };
    let energy_wh = energy("energy_now", "charge_now");
    let energy_full_wh = energy("energy_full", "charge_full");
    let energy_design_wh = energy("energy_full_design", "charge_full_design");

    let health = match (energy_full_wh, energy_design_wh) {
        (Some(full), Some(design)) if design > 0.0 => Some((full / design).clamp(0.0, 1.0) as f32),
        _ => None,
    };

    // Power in µW, or current in µA times the voltage.
    let power_w = read_abs("power_now")
        .map(|uw| uw as f64 / 1_000_000.0)
        .or_else(|| Some(read_abs("current_now")? as f64 * voltage_v? / 1_000_000.0));

    let time_remaining_secs = match (state, power_w, energy_wh, energy_full_wh) {
        (BatteryState::Discharging, Some(p), Some(now), _) if p > 0.1 => {
            Some((now / p * 3600.0) as u64)
        }
        (BatteryState::Charging, Some(p), Some(now), Some(full)) if p > 0.1 && full > now => {
            Some(((full - now) / p * 3600.0) as u64)
        }
        _ => None,
    };

    BatteryDevice {
        name: det.name.clone(),
        model: det.model.clone(),
        technology: det.technology.clone(),
        state,
        capacity,
        energy_wh,
        energy_full_wh,
        energy_design_wh,
        health,
        power_w,
        voltage_v,
        time_remaining_secs,
        cycle_count,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn battery_state_parsing() {
        assert_eq!("Charging".parse(), Ok(BatteryState::Charging));
        assert_eq!("Discharging".parse(), Ok(BatteryState::Discharging));
        assert_eq!("Not charging".parse(), Ok(BatteryState::NotCharging));
        assert_eq!("Full".parse(), Ok(BatteryState::Full));
        assert_eq!("other".parse(), Ok(BatteryState::Unknown));
    }
}
