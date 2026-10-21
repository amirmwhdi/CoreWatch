//! Battery and power supply module: capacity, charging state, health, rate, and time remaining.

use crate::{CollectError, Collector, Snapshot, SysRoot};
use std::fs;
use std::path::PathBuf;

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

struct BatteryDetector {
    name: String,
    path: PathBuf,
}

pub struct BatteryCollector {
    detectors: Vec<BatteryDetector>,
    ac_paths: Vec<PathBuf>,
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
                let type_str = fs::read_to_string(path.join("type"))
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();

                if type_str == "battery" || name.starts_with("BAT") {
                    detectors.push(BatteryDetector { name, path });
                } else if type_str == "mains" || name.starts_with("AC") || name.starts_with("ADP") {
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
        Ok(Self {
            detectors,
            ac_paths,
        })
    }
}

impl Collector for BatteryCollector {
    fn name(&self) -> &'static str {
        "battery"
    }

    fn collect(&mut self, _root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        let mut batteries = Vec::new();

        let ac_online = self.ac_paths.iter().find_map(|p| {
            fs::read_to_string(p)
                .ok()
                .and_then(|s| s.trim().parse::<u8>().ok())
                .map(|val| val == 1)
        });

        for det in &self.detectors {
            let read_str = |file: &str| -> Option<String> {
                fs::read_to_string(det.path.join(file))
                    .ok()
                    .map(|s| s.trim().to_string())
            };
            let read_u64 =
                |file: &str| -> Option<u64> { read_str(file).and_then(|s| s.parse().ok()) };

            let status_str = read_str("status").unwrap_or_else(|| "Unknown".to_string());
            let state = status_str.parse().unwrap_or(BatteryState::Unknown);

            let capacity_val = read_u64("capacity").unwrap_or(0);
            let capacity = (capacity_val as f32 / 100.0).clamp(0.0, 1.0);

            let model = read_str("model_name")
                .or_else(|| read_str("manufacturer"))
                .unwrap_or_else(|| det.name.clone());

            let technology = read_str("technology").unwrap_or_else(|| "Li-ion".to_string());
            let cycle_count = read_u64("cycle_count").map(|c| c as u32);

            let voltage_v = read_u64("voltage_now").map(|uv| uv as f64 / 1_000_000.0);

            // Energy in µWh, or fallback to charge in µAh * voltage
            let energy_now_uwh = read_u64("energy_now").or_else(|| {
                let charge = read_u64("charge_now")?;
                let v = voltage_v?;
                Some((charge as f64 * v) as u64)
            });
            let energy_full_uwh = read_u64("energy_full").or_else(|| {
                let charge = read_u64("charge_full")?;
                let v = voltage_v?;
                Some((charge as f64 * v) as u64)
            });
            let energy_design_uwh = read_u64("energy_full_design").or_else(|| {
                let charge = read_u64("charge_full_design")?;
                let v = voltage_v?;
                Some((charge as f64 * v) as u64)
            });

            let energy_wh = energy_now_uwh.map(|e| e as f64 / 1_000_000.0);
            let energy_full_wh = energy_full_uwh.map(|e| e as f64 / 1_000_000.0);
            let energy_design_wh = energy_design_uwh.map(|e| e as f64 / 1_000_000.0);

            let health = match (energy_full_wh, energy_design_wh) {
                (Some(full), Some(design)) if design > 0.0 => {
                    Some((full / design).clamp(0.0, 1.0) as f32)
                }
                _ => None,
            };

            // Power in µW, or fallback to current in µA * voltage
            let power_w = read_u64("power_now")
                .map(|uw| uw as f64 / 1_000_000.0)
                .or_else(|| {
                    let current_ua = read_u64("current_now")?;
                    let v = voltage_v?;
                    Some(current_ua as f64 * v / 1_000_000.0)
                });

            // Estimated time remaining
            let time_remaining_secs = match (state, power_w, energy_wh, energy_full_wh) {
                (BatteryState::Discharging, Some(p), Some(now), _) if p > 0.1 => {
                    Some((now / p * 3600.0) as u64)
                }
                (BatteryState::Charging, Some(p), Some(now), Some(full))
                    if p > 0.1 && full > now =>
                {
                    Some(((full - now) / p * 3600.0) as u64)
                }
                _ => None,
            };

            batteries.push(BatteryDevice {
                name: det.name.clone(),
                model,
                technology,
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
            });
        }

        out.battery = Some(BatterySample {
            ac_online,
            batteries,
        });
        Ok(())
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
