//! Sensors module: temperatures and fan speeds from `/sys/class/hwmon`.
//!
//! Each `hwmonN` directory is one chip (the CPU's thermal sensor, an NVMe
//! drive, the motherboard's fan controller). Which files a chip has is found
//! once and kept, so a tick only reads the values themselves.

use crate::{CollectError, Collector, Snapshot, SysRoot};
use std::fs;
use std::path::{Path, PathBuf};

/// Chips are rarely added (a USB drive with a sensor); look for new ones once
/// every this many ticks.
pub const RESCAN_EVERY: u32 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChipKind {
    Cpu,
    Gpu,
    Storage,
    Board,
    Network,
    Battery,
    Other,
}

/// The page a sensor belongs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Home {
    Cpu,
    Gpu,
    Disk,
    Network,
    Battery,
    /// Everything else: motherboard, case fans, ACPI zones.
    Sensors,
}

impl Chip {
    /// Whether temperature `index` is the one the chip's page already shows
    /// on its own: `temp1` of a graphics card ("edge" on AMD) on the
    /// Graphics page, and of a drive on its Disks entry.
    pub fn shown_by_page(&self, index: u32) -> bool {
        index == 1 && matches!(self.kind, ChipKind::Gpu | ChipKind::Storage)
    }

    /// Where the sensor with `label` on this chip belongs. A motherboard
    /// sensor labeled for the processor (`CPU_FAN`, `CPUTIN`) goes with the
    /// processor.
    pub fn home(&self, label: Option<&str>) -> Home {
        match self.kind {
            ChipKind::Cpu => Home::Cpu,
            ChipKind::Gpu => Home::Gpu,
            ChipKind::Storage => Home::Disk,
            ChipKind::Network => Home::Network,
            ChipKind::Battery => Home::Battery,
            ChipKind::Board | ChipKind::Other => {
                let cpu = label.is_some_and(|l| l.to_ascii_lowercase().starts_with("cpu"));
                if cpu {
                    Home::Cpu
                } else {
                    Home::Sensors
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Temperature {
    /// The driver's label (`"Tctl"`, `"Package id 0"`, `"Composite"`), or
    /// `None` when it gives none.
    pub label: Option<String>,
    /// The `N` of the driver's `tempN_input`.
    pub index: u32,
    pub celsius: f32,
    /// The temperature the chip calls critical, when it reports one.
    pub critical: Option<f32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Fan {
    pub label: Option<String>,
    pub rpm: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Chip {
    /// The hwmon driver name, e.g. `"k10temp"`, `"nvme"`, `"nct6798"`.
    pub driver: String,
    pub kind: ChipKind,
    /// Model of the device the chip belongs to (an NVMe or SATA drive).
    pub model: Option<String>,
    pub temperatures: Vec<Temperature>,
    pub fans: Vec<Fan>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SensorSample {
    /// Sorted by kind (CPU first), then by driver name.
    pub chips: Vec<Chip>,
}

impl SensorSample {
    /// The processor temperature: the package or control sensor when the
    /// driver labels one, otherwise its first sensor.
    pub fn cpu_celsius(&self) -> Option<f32> {
        let chip = self.chips.iter().find(|c| c.kind == ChipKind::Cpu)?;
        let preferred = ["Tctl", "Tdie", "Package id 0"];
        chip.temperatures
            .iter()
            .find(|t| t.label.as_deref().is_some_and(|l| preferred.contains(&l)))
            .or_else(|| chip.temperatures.first())
            .map(|t| t.celsius)
    }

    /// The highest temperature of any sensor.
    pub fn hottest_celsius(&self) -> Option<f32> {
        self.chips
            .iter()
            .flat_map(|c| &c.temperatures)
            .map(|t| t.celsius)
            .max_by(f32::total_cmp)
    }
}

/// The files of one sensor, found once.
#[derive(Debug, Clone)]
struct Input {
    label: Option<String>,
    index: u32,
    path: PathBuf,
    critical: Option<f32>,
}

#[derive(Debug, Clone)]
struct ChipPlan {
    driver: String,
    kind: ChipKind,
    model: Option<String>,
    temps: Vec<Input>,
    fans: Vec<Input>,
}

pub struct SensorCollector {
    plan: Vec<ChipPlan>,
    rescan_countdown: u32,
}

impl SensorCollector {
    /// Fails when there is no hwmon chip at all, which disables the module.
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        let plan = scan(root)?;
        if plan.is_empty() {
            return Err(CollectError::Io {
                path: root.sys_path("class/hwmon"),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "no sensors"),
            });
        }
        Ok(Self {
            plan,
            rescan_countdown: RESCAN_EVERY,
        })
    }
}

impl Collector for SensorCollector {
    fn name(&self) -> &'static str {
        "sensors"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        if self.rescan_countdown == 0 {
            // A failed rescan keeps the chips already known.
            if let Ok(plan) = scan(root) {
                self.plan = plan;
            }
            self.rescan_countdown = RESCAN_EVERY;
        }
        self.rescan_countdown -= 1;

        let chips = self
            .plan
            .iter()
            .map(|chip| Chip {
                driver: chip.driver.clone(),
                kind: chip.kind,
                model: chip.model.clone(),
                // A sensor that fails this tick (a drive asleep) is left out.
                temperatures: chip
                    .temps
                    .iter()
                    .filter_map(|t| {
                        let celsius = read_milli(&t.path)?;
                        Some(Temperature {
                            label: t.label.clone(),
                            index: t.index,
                            celsius,
                            critical: t.critical,
                        })
                    })
                    .collect(),
                fans: chip
                    .fans
                    .iter()
                    .filter_map(|f| {
                        Some(Fan {
                            label: f.label.clone(),
                            rpm: read_number(&f.path)?.try_into().ok()?,
                        })
                    })
                    .collect(),
            })
            .filter(|c| !c.temperatures.is_empty() || !c.fans.is_empty())
            .collect();
        out.sensors = Some(SensorSample { chips });
        Ok(())
    }
}

/// Every chip with at least one temperature or fan input.
fn scan(root: &SysRoot) -> Result<Vec<ChipPlan>, CollectError> {
    let dir = root.sys_path("class/hwmon");
    let entries = fs::read_dir(&dir).map_err(|source| CollectError::Io {
        path: dir.clone(),
        source,
    })?;
    let mut chips: Vec<ChipPlan> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|path| scan_chip(&path))
        .collect();
    chips.sort_by(|a, b| (a.kind, &a.driver).cmp(&(b.kind, &b.driver)));
    Ok(chips)
}

fn scan_chip(dir: &Path) -> Option<ChipPlan> {
    let driver = read_trimmed(&dir.join("name"))?;
    let mut temps = Vec::new();
    let mut fans = Vec::new();
    let mut names: Vec<String> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok()?.file_name().into_string().ok())
        .filter(|n| n.ends_with("_input"))
        .collect();
    // temp2 before temp10.
    names.sort_by_key(|n| {
        let digits: String = n.chars().filter(char::is_ascii_digit).collect();
        (n.chars().next(), digits.parse::<u32>().unwrap_or(0))
    });
    for name in names {
        let base = name.trim_end_matches("_input");
        let index = base
            .trim_start_matches(|c: char| c.is_ascii_alphabetic())
            .parse()
            .unwrap_or(0);
        let input = Input {
            index,
            label: read_trimmed(&dir.join(format!("{base}_label"))).filter(|l| !l.is_empty()),
            path: dir.join(&name),
            critical: read_milli(&dir.join(format!("{base}_crit")))
                .filter(|c| *c > 0.0 && *c < 200.0),
        };
        if base.starts_with("temp") {
            temps.push(input);
        } else if base.starts_with("fan") {
            fans.push(input);
        }
    }
    if temps.is_empty() && fans.is_empty() {
        return None;
    }
    let kind = chip_kind(&driver);
    let model = (kind == ChipKind::Storage)
        .then(|| read_trimmed(&dir.join("device/model")))
        .flatten()
        .filter(|m| !m.is_empty());
    Some(ChipPlan {
        driver,
        kind,
        model,
        temps,
        fans,
    })
}

/// What a hwmon driver measures, from its name.
pub fn chip_kind(driver: &str) -> ChipKind {
    match driver {
        "k10temp" | "coretemp" | "zenpower" | "cpu_thermal" | "cpu-thermal" | "soc_thermal"
        | "via_cputemp" => ChipKind::Cpu,
        "amdgpu" | "radeon" | "nouveau" | "i915" | "xe" => ChipKind::Gpu,
        "nvme" | "drivetemp" => ChipKind::Storage,
        d if d.starts_with("iwlwifi") || d.starts_with("mt79") || d.starts_with("ath") => {
            ChipKind::Network
        }
        d if d.starts_with("nct")
            || d.starts_with("it87")
            || d.starts_with("asus")
            || d.starts_with("thinkpad")
            || d.starts_with("dell")
            || d == "acpitz"
            || d == "applesmc"
            || d == "pch_cannonlake"
            || d.starts_with("pch_") =>
        {
            ChipKind::Board
        }
        d if d.starts_with("BAT") || d == "battery" => ChipKind::Battery,
        _ => ChipKind::Other,
    }
}

fn read_trimmed(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_owned())
}

fn read_number(path: &Path) -> Option<i64> {
    read_trimmed(path)?.parse().ok()
}

/// hwmon temperatures are in millidegrees Celsius.
fn read_milli(path: &Path) -> Option<f32> {
    read_number(path).map(|v| v as f32 / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drivers_map_to_kinds() {
        assert_eq!(chip_kind("k10temp"), ChipKind::Cpu);
        assert_eq!(chip_kind("coretemp"), ChipKind::Cpu);
        assert_eq!(chip_kind("nvme"), ChipKind::Storage);
        assert_eq!(chip_kind("nct6798"), ChipKind::Board);
        assert_eq!(chip_kind("iwlwifi_1"), ChipKind::Network);
        assert_eq!(chip_kind("BAT0"), ChipKind::Battery);
        assert_eq!(chip_kind("acpi_fan"), ChipKind::Other);
    }

    #[test]
    fn sensors_go_to_their_pages() {
        let chip = |driver: &str| Chip {
            driver: driver.into(),
            kind: chip_kind(driver),
            model: None,
            temperatures: vec![],
            fans: vec![],
        };
        assert_eq!(chip("k10temp").home(Some("Tctl")), Home::Cpu);
        assert_eq!(chip("nvme").home(Some("Composite")), Home::Disk);
        assert_eq!(chip("nct6798").home(Some("CPU_FAN")), Home::Cpu);
        assert_eq!(chip("nct6798").home(Some("SYSTIN")), Home::Sensors);
        assert_eq!(chip("nct6798").home(None), Home::Sensors);
        // The Graphics and Disks pages show temp1 themselves.
        assert!(chip("amdgpu").shown_by_page(1));
        assert!(!chip("amdgpu").shown_by_page(2));
        assert!(chip("nvme").shown_by_page(1));
        assert!(!chip("k10temp").shown_by_page(1));
    }

    #[test]
    fn cpu_temperature_prefers_the_package_sensor() {
        let t = |label: &str, celsius| Temperature {
            label: Some(label.into()),
            index: 1,
            celsius,
            critical: None,
        };
        let sample = SensorSample {
            chips: vec![Chip {
                driver: "coretemp".into(),
                kind: ChipKind::Cpu,
                model: None,
                temperatures: vec![t("Core 0", 50.0), t("Package id 0", 55.0)],
                fans: vec![],
            }],
        };
        assert_eq!(sample.cpu_celsius(), Some(55.0));
        assert_eq!(sample.hottest_celsius(), Some(55.0));
    }
}
