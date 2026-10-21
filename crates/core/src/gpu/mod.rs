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
    /// GPU usage between 0.0 and 1.0.
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
    /// Aggregate usage across all GPUs (highest or average).
    pub fn max_usage(&self) -> Option<f32> {
        self.devices
            .iter()
            .filter_map(|d| d.usage)
            .max_by(|a, b| a.total_cmp(b))
    }
}

struct GpuDetector {
    card_name: String,
    device_path: PathBuf,
    vendor: GpuVendor,
    name: String,
    driver: Option<String>,
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

                detectors.push(GpuDetector {
                    card_name: card,
                    device_path: dev_link,
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
            let mut usage = None;
            let mut vram_used = None;
            let mut vram_total = None;
            let mut freq_mhz = None;
            let mut max_freq_mhz = None;
            let mut temp_c = None;

            match det.vendor {
                GpuVendor::Amd => {
                    // AMD exposes gpu_busy_percent, mem_info_vram_total / used
                    if let Ok(s) = fs::read_to_string(det.device_path.join("gpu_busy_percent")) {
                        if let Ok(pct) = s.trim().parse::<f32>() {
                            usage = Some((pct / 100.0).clamp(0.0, 1.0));
                        }
                    }
                    if let Ok(s) = fs::read_to_string(det.device_path.join("mem_info_vram_total")) {
                        vram_total = s.trim().parse::<u64>().ok();
                    }
                    if let Ok(s) = fs::read_to_string(det.device_path.join("mem_info_vram_used")) {
                        vram_used = s.trim().parse::<u64>().ok();
                    }
                }
                GpuVendor::Intel => {
                    // Intel exposes gt_act_freq_mhz, gt_cur_freq_mhz, gt_max_freq_mhz
                    // and rc6 residency
                    let drm_card = det.device_path.join("drm").join(&det.card_name);
                    let gt_dir = if drm_card.join("gt_act_freq_mhz").exists() {
                        drm_card
                    } else if det
                        .device_path
                        .join(&det.card_name)
                        .join("gt_act_freq_mhz")
                        .exists()
                    {
                        det.device_path.join(&det.card_name)
                    } else {
                        det.device_path
                            .join("drm")
                            .join(&det.card_name)
                            .join("gt")
                            .join("gt0")
                    };

                    let act = fs::read_to_string(gt_dir.join("gt_act_freq_mhz"))
                        .or_else(|_| fs::read_to_string(gt_dir.join("rps_act_freq_mhz")))
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok());

                    let max = fs::read_to_string(gt_dir.join("gt_max_freq_mhz"))
                        .or_else(|_| fs::read_to_string(gt_dir.join("rps_max_freq_mhz")))
                        .ok()
                        .and_then(|s| s.trim().parse::<u32>().ok());

                    if let (Some(act_val), Some(max_val)) = (act, max) {
                        freq_mhz = Some(act_val);
                        max_freq_mhz = Some(max_val);
                        if max_val > 0 {
                            usage = Some((act_val as f32 / max_val as f32).clamp(0.0, 1.0));
                        }
                    } else if let Some(act_val) = act {
                        freq_mhz = Some(act_val);
                    }
                }
                _ => {}
            }

            // Temperature from hwmon
            let hwmon_base = det.device_path.join("hwmon");
            if let Ok(hwmon_entries) = fs::read_dir(hwmon_base) {
                for h_entry in hwmon_entries.flatten() {
                    let temp_file = h_entry.path().join("temp1_input");
                    if let Ok(s) = fs::read_to_string(temp_file) {
                        if let Ok(millicelsius) = s.trim().parse::<u32>() {
                            temp_c = Some(millicelsius / 1000);
                            break;
                        }
                    }
                }
            }

            devices.push(GpuDevice {
                id: det.card_name.clone(),
                name: det.name.clone(),
                vendor: det.vendor,
                driver: det.driver.clone(),
                usage,
                vram_used,
                vram_total,
                freq_mhz,
                max_freq_mhz,
                temp_c,
            });
        }

        out.gpu = Some(GpuSample { devices });
        Ok(())
    }
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
