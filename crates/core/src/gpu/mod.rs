//! GPU module: detect AMD, Intel, and NVIDIA graphics devices and monitor
//! usage, memory (VRAM), clock frequency, and temperature.

use crate::{CollectError, Collector, Snapshot, SysRoot};
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GpuVendor {
    Intel,
    Amd,
    Nvidia,
    Other,
}

impl GpuVendor {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Intel => "Intel",
            Self::Amd => "AMD",
            Self::Nvidia => "NVIDIA",
            Self::Other => "GPU",
        }
    }
}

/// A single detected graphics card / GPU.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuDevice {
    /// Card identifier, e.g. "card0" or "card1".
    pub id: String,
    pub name: String,
    pub vendor: GpuVendor,
    pub driver: Option<String>,
    /// GPU busy share between 0.0 and 1.0, when the driver reports one
    /// (amdgpu does; i915/xe and the NVIDIA driver do not expose it in sysfs).
    pub usage: Option<f32>,
    /// VRAM in bytes: used and total.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// Clock frequencies in MHz.
    pub freq_mhz: Option<u32>,
    pub max_freq_mhz: Option<u32>,
    /// Temperature in degrees Celsius.
    pub temp_c: Option<u32>,
}

impl GpuDevice {
    /// Current clock as a share of the maximum clock.
    pub fn freq_fraction(&self) -> Option<f32> {
        match (self.freq_mhz, self.max_freq_mhz) {
            (Some(cur), Some(max)) if max > 0 => Some((cur as f32 / max as f32).clamp(0.0, 1.0)),
            _ => None,
        }
    }

    pub fn vram_fraction(&self) -> Option<f32> {
        match (self.vram_used, self.vram_total) {
            (Some(used), Some(total)) if total > 0 => {
                Some((used as f64 / total as f64).clamp(0.0, 1.0) as f32)
            }
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GpuSample {
    pub devices: Vec<GpuDevice>,
}

impl GpuSample {
    /// The busiest GPU's usage.
    pub fn max_usage(&self) -> Option<f32> {
        self.devices
            .iter()
            .filter_map(|d| d.usage)
            .max_by(|a, b| a.total_cmp(b))
    }
}

/// One card with the files it is read from, resolved once at start-up so a
/// tick only reads files and never searches for them.
struct GpuDetector {
    card_name: String,
    vendor: GpuVendor,
    name: String,
    driver: Option<String>,
    /// AMD: `gpu_busy_percent`, `mem_info_vram_used`, `mem_info_vram_total`.
    busy: Option<PathBuf>,
    vram_used: Option<PathBuf>,
    vram_total: Option<PathBuf>,
    /// Intel: actual and maximum GT frequency.
    act_freq: Option<PathBuf>,
    max_freq: Option<PathBuf>,
    temp: Option<PathBuf>,
}

pub struct GpuCollector {
    detectors: Vec<GpuDetector>,
}

impl GpuCollector {
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        let drm_dir = root.sys_path("class/drm");
        let mut detectors = Vec::new();

        if let Ok(entries) = fs::read_dir(&drm_dir) {
            let mut card_names: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter_map(|e| {
                    let file_name = e.file_name().to_string_lossy().to_string();
                    if file_name.starts_with("card") && !file_name.contains('-') {
                        Some(file_name)
                    } else {
                        None
                    }
                })
                .collect();
            card_names.sort();

            for card in card_names {
                let card_path = drm_dir.join(&card);
                let dev_link = card_path.join("device");
                if !dev_link.exists() {
                    continue;
                }

                let vendor_id = fs::read_to_string(dev_link.join("vendor"))
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();
                let device_id = fs::read_to_string(dev_link.join("device"))
                    .unwrap_or_default()
                    .trim()
                    .to_lowercase();

                let vendor = match vendor_id.as_str() {
                    "0x8086" => GpuVendor::Intel,
                    "0x1002" => GpuVendor::Amd,
                    "0x10de" => GpuVendor::Nvidia,
                    _ => GpuVendor::Other,
                };

                let driver = fs::read_link(dev_link.join("driver"))
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().to_string()));

                let name = match vendor {
                    GpuVendor::Intel => "Intel Graphics".to_string(),
                    GpuVendor::Amd => "AMD Radeon Graphics".to_string(),
                    GpuVendor::Nvidia => "NVIDIA Graphics".to_string(),
                    GpuVendor::Other => {
                        if !device_id.is_empty() {
                            format!("GPU ({}:{})", vendor_id, device_id)
                        } else {
                            "Graphics Device".to_string()
                        }
                    }
                };

                let existing = |p: PathBuf| p.exists().then_some(p);
                let (mut act_freq, mut max_freq) = (None, None);
                if vendor == GpuVendor::Intel {
                    let gt_dir = [
                        dev_link.join("drm").join(&card),
                        dev_link.join(&card),
                        dev_link.join("drm").join(&card).join("gt").join("gt0"),
                    ]
                    .into_iter()
                    .find(|d| {
                        d.join("gt_act_freq_mhz").exists() || d.join("rps_act_freq_mhz").exists()
                    });
                    if let Some(dir) = gt_dir {
                        act_freq = existing(dir.join("gt_act_freq_mhz"))
                            .or_else(|| existing(dir.join("rps_act_freq_mhz")));
                        max_freq = existing(dir.join("gt_max_freq_mhz"))
                            .or_else(|| existing(dir.join("rps_max_freq_mhz")));
                    }
                }
                let amd = |file: &str| {
                    if vendor == GpuVendor::Amd {
                        existing(dev_link.join(file))
                    } else {
                        None
                    }
                };
                let temp = fs::read_dir(dev_link.join("hwmon")).ok().and_then(|dirs| {
                    let mut paths: Vec<PathBuf> = dirs
                        .flatten()
                        .map(|d| d.path().join("temp1_input"))
                        .filter(|p| p.exists())
                        .collect();
                    paths.sort();
                    paths.into_iter().next()
                });

                detectors.push(GpuDetector {
                    busy: amd("gpu_busy_percent"),
                    vram_used: amd("mem_info_vram_used"),
                    vram_total: amd("mem_info_vram_total"),
                    act_freq,
                    max_freq,
                    temp,
                    card_name: card,
                    vendor,
                    name,
                    driver,
                });
            }
        }

        if detectors.is_empty() {
            return Err(CollectError::parse(
                "sys/class/drm",
                "no supported GPU detected",
            ));
        }

        Ok(Self { detectors })
    }
}

impl Collector for GpuCollector {
    fn name(&self) -> &'static str {
        "gpu"
    }

    fn collect(&mut self, _root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        let mut devices = Vec::new();

        for det in &self.detectors {
            let freq_mhz = read_num::<u32>(&det.act_freq);
            let max_freq_mhz = read_num::<u32>(&det.max_freq).filter(|m| *m > 0);
            devices.push(GpuDevice {
                id: det.card_name.clone(),
                name: det.name.clone(),
                vendor: det.vendor,
                driver: det.driver.clone(),
                usage: read_num::<f32>(&det.busy).map(|pct| (pct / 100.0).clamp(0.0, 1.0)),
                vram_used: read_num(&det.vram_used),
                vram_total: read_num(&det.vram_total),
                freq_mhz,
                max_freq_mhz,
                temp_c: read_num::<u32>(&det.temp).map(|mc| mc / 1000),
            });
        }

        out.gpu = Some(GpuSample { devices });
        Ok(())
    }
}

fn read_num<T: std::str::FromStr>(path: &Option<PathBuf>) -> Option<T> {
    fs::read_to_string(path.as_ref()?).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vram_fraction_calculation() {
        let dev = GpuDevice {
            id: "card0".into(),
            name: "Test GPU".into(),
            vendor: GpuVendor::Amd,
            driver: Some("amdgpu".into()),
            usage: Some(0.5),
            vram_used: Some(2 * 1024 * 1024 * 1024),
            vram_total: Some(8 * 1024 * 1024 * 1024),
            freq_mhz: Some(1500),
            max_freq_mhz: Some(2000),
            temp_c: Some(55),
        };
        assert_eq!(dev.vram_fraction(), Some(0.25));

        let zero_total = GpuDevice {
            vram_total: Some(0),
            ..dev.clone()
        };
        assert_eq!(zero_total.vram_fraction(), None);
    }

    #[test]
    fn gpu_vendor_labels() {
        assert_eq!(GpuVendor::Intel.as_str(), "Intel");
        assert_eq!(GpuVendor::Amd.as_str(), "AMD");
        assert_eq!(GpuVendor::Nvidia.as_str(), "NVIDIA");
        assert_eq!(GpuVendor::Other.as_str(), "GPU");
    }
}
