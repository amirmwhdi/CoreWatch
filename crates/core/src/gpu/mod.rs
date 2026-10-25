//! GPU module: every graphics card with its load, memory, clocks,
//! temperature, power and fan.
//!
//! Cards are found once at start: display controllers on the PCI bus, plus
//! DRM cards that are not on PCI (ARM boards). Where each value comes from
//! depends on the driver:
//!
//! - amdgpu: `gpu_busy_percent`, `mem_info_vram_*`, the clock levels in
//!   `pp_dpm_sclk` / `pp_dpm_mclk` (the current one is marked `*`), and the
//!   hwmon chip for the measured clock, temperature, power and fan.
//! - i915 and xe (Intel): the GT frequency; Intel reports no load in sysfs.
//! - nouveau: the hwmon chip only (temperature, fan, power).
//! - nvidia (proprietary): nothing in sysfs; NVML ([`nvml`]) has it all.
//!
//! A laptop's discrete GPU sleeps when unused, and reading its files or
//! asking NVML would wake it. A card whose `power/runtime_status` is
//! `suspended` is reported as asleep and not read.

mod nvml;
pub mod pci;

use crate::{CollectError, Collector, Snapshot, SysRoot};
use nvml::{Nvml, NvmlDevice, NvmlReading};
use pci::PciIds;
use std::fs;
use std::path::{Path, PathBuf};

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

    fn from_pci(vendor: &str) -> Self {
        match vendor {
            "8086" => Self::Intel,
            "1002" => Self::Amd,
            "10de" => Self::Nvidia,
            _ => Self::Other,
        }
    }
}

/// One graphics card in one sample. Values the driver does not report are
/// `None`, and so is everything but the name while the card is asleep.
#[derive(Debug, Clone, PartialEq)]
pub struct GpuDevice {
    /// PCI address (`"0000:01:00.0"`), or the device folder's name off PCI.
    pub id: String,
    /// Model name, e.g. `"AMD Radeon RX 580 Armor 4G OC"`.
    pub name: String,
    pub vendor: GpuVendor,
    pub driver: Option<String>,
    /// Powered down by runtime power management.
    pub asleep: bool,
    /// Share of time the GPU was busy, 0.0..=1.0.
    pub usage: Option<f32>,
    /// Dedicated video memory in bytes.
    pub vram_used: Option<u64>,
    pub vram_total: Option<u64>,
    /// Core clock in MHz, now and at most.
    pub freq_mhz: Option<u32>,
    pub max_freq_mhz: Option<u32>,
    /// Memory clock in MHz, now and at most.
    pub mem_freq_mhz: Option<u32>,
    pub mem_max_freq_mhz: Option<u32>,
    /// Temperature in °C.
    pub temp_c: Option<u32>,
    /// Power draw and limit in watts.
    pub power_w: Option<f32>,
    pub power_cap_w: Option<f32>,
    pub fan_rpm: Option<u32>,
    /// Fan speed as a share of its maximum (NVIDIA reports only this).
    pub fan_percent: Option<f32>,
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
    /// Discrete cards before Intel's integrated ones, then by address.
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

/// The sysfs files of one card, found once.
#[derive(Default)]
struct Files {
    busy: Option<PathBuf>,
    vram_used: Option<PathBuf>,
    vram_total: Option<PathBuf>,
    /// amdgpu clock levels (`pp_dpm_sclk`, `pp_dpm_mclk`).
    sclk_levels: Option<PathBuf>,
    mclk_levels: Option<PathBuf>,
    /// Measured clocks in Hz (amdgpu hwmon `freq1_input`, `freq2_input`).
    sclk_hz: Option<PathBuf>,
    mclk_hz: Option<PathBuf>,
    /// Intel GT clock in MHz, now and at most.
    act_mhz: Option<PathBuf>,
    max_mhz: Option<PathBuf>,
    temp: Option<PathBuf>,
    /// Microwatts.
    power: Option<PathBuf>,
    power_cap: Option<PathBuf>,
    fan: Option<PathBuf>,
}

struct Card {
    id: String,
    name: String,
    vendor: GpuVendor,
    driver: Option<String>,
    runtime_status: PathBuf,
    files: Files,
    nvml: Option<NvmlDevice>,
}

pub struct GpuCollector {
    cards: Vec<Card>,
    // Declared after `cards`: their NVML handles go before NVML shuts down.
    nvml: Option<Nvml>,
}

impl GpuCollector {
    /// Fails when the machine has no graphics card, which disables the module.
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        let dirs = card_dirs(root);
        if dirs.is_empty() {
            return Err(CollectError::parse(
                "sys/class/drm",
                "no graphics card found",
            ));
        }
        let pci_ids = read_pci_ids(root);
        let mut nvml = None;
        let mut cards: Vec<Card> = dirs
            .into_iter()
            .map(|dir| {
                let ids = PciIds::read(&dir);
                let vendor = GpuVendor::from_pci(&ids.vendor);
                let driver = fs::read_link(dir.join("driver"))
                    .ok()
                    .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()));
                let id = dir
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let mut nvml_device = None;
                let mut name = None;
                if driver.as_deref() == Some("nvidia") {
                    if nvml.is_none() {
                        nvml = Some(Nvml::open());
                    }
                    if let Some(Some(lib)) = &nvml {
                        nvml_device = lib.device(&id);
                        name = nvml_device.as_ref().and_then(|d| lib.name(d));
                    }
                }
                let name = name
                    .or_else(|| {
                        let model = pci::model_name(pci_ids.as_deref()?, &ids)?;
                        Some(with_vendor(vendor, &model))
                    })
                    .unwrap_or_else(|| format!("{} Graphics", vendor.as_str()));
                Card {
                    files: find_files(&dir, vendor, driver.as_deref()),
                    runtime_status: dir.join("power/runtime_status"),
                    id,
                    name,
                    vendor,
                    driver,
                    nvml: nvml_device,
                }
            })
            .collect();
        cards.sort_by(|a, b| {
            (a.vendor == GpuVendor::Intel, &a.id).cmp(&(b.vendor == GpuVendor::Intel, &b.id))
        });
        Ok(Self {
            cards,
            nvml: nvml.flatten(),
        })
    }
}

/// "AMD Radeon RX 580", without doubling a vendor the name already has.
fn with_vendor(vendor: GpuVendor, model: &str) -> String {
    let prefix = vendor.as_str();
    if vendor == GpuVendor::Other || model.starts_with(prefix) {
        model.to_owned()
    } else {
        format!("{prefix} {model}")
    }
}

/// Device folders of every graphics card: PCI display controllers (class
/// 0x03xxxx), then DRM cards on other buses.
fn card_dirs(root: &SysRoot) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Ok(entries) = fs::read_dir(root.sys_path("bus/pci/devices")) {
        for entry in entries.flatten() {
            let class = fs::read_to_string(entry.path().join("class")).unwrap_or_default();
            if class.trim().starts_with("0x03") {
                dirs.extend(entry.path().canonicalize());
            }
        }
    }
    if let Ok(entries) = fs::read_dir(root.sys_path("class/drm")) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // `card0-eDP-1` is a connector, not a card.
            if !name.starts_with("card") || name.contains('-') {
                continue;
            }
            if let Ok(dir) = entry.path().join("device").canonicalize() {
                if !dirs.contains(&dir) {
                    dirs.push(dir);
                }
            }
        }
    }
    dirs
}

fn read_pci_ids(root: &SysRoot) -> Option<String> {
    if root.is_host() {
        pci::PCI_IDS_PATHS
            .iter()
            .find_map(|p| fs::read_to_string(p).ok())
    } else {
        // A test tree keeps its own copy next to `sys`.
        let base = root.sys_path("").parent()?.to_owned();
        fs::read_to_string(base.join("usr/share/hwdata/pci.ids")).ok()
    }
}

fn find_files(dir: &Path, vendor: GpuVendor, driver: Option<&str>) -> Files {
    let existing = |p: PathBuf| p.is_file().then_some(p);
    let mut files = Files::default();
    if let Some(hwmon) = first_child(&dir.join("hwmon"), "hwmon") {
        let h = |f: &str| existing(hwmon.join(f));
        files.temp = h("temp1_input");
        files.power = h("power1_average").or_else(|| h("power1_input"));
        files.power_cap = h("power1_cap");
        files.fan = h("fan1_input");
        if vendor == GpuVendor::Amd {
            files.sclk_hz = h("freq1_input");
            files.mclk_hz = h("freq2_input");
        }
    }
    match (vendor, driver) {
        (GpuVendor::Amd, _) => {
            let f = |name: &str| existing(dir.join(name));
            files.busy = f("gpu_busy_percent");
            files.vram_used = f("mem_info_vram_used");
            files.vram_total = f("mem_info_vram_total");
            files.sclk_levels = f("pp_dpm_sclk");
            files.mclk_levels = f("pp_dpm_mclk");
        }
        (GpuVendor::Intel, Some("xe")) => {
            let freq = dir.join("tile0/gt0/freq0");
            files.act_mhz = existing(freq.join("act_freq"));
            files.max_mhz = existing(freq.join("max_freq"));
        }
        (GpuVendor::Intel, _) => {
            if let Some(card) = first_child(&dir.join("drm"), "card") {
                for gt in [card.clone(), card.join("gt/gt0")] {
                    let act = existing(gt.join("gt_act_freq_mhz"))
                        .or_else(|| existing(gt.join("rps_act_freq_mhz")));
                    if act.is_some() {
                        files.act_mhz = act;
                        files.max_mhz = existing(gt.join("gt_max_freq_mhz"))
                            .or_else(|| existing(gt.join("rps_max_freq_mhz")));
                        break;
                    }
                }
            }
        }
        _ => {}
    }
    files
}

/// The first entry of `dir` whose name starts with `prefix`, by name.
fn first_child(dir: &Path, prefix: &str) -> Option<PathBuf> {
    let mut children: Vec<PathBuf> = fs::read_dir(dir)
        .ok()?
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with(prefix))
        .map(|e| e.path())
        .collect();
    children.sort();
    children.into_iter().next()
}

/// The current and the highest clock from amdgpu's level list:
///
/// ```text
/// 0: 300Mhz
/// 1: 1366Mhz *
/// ```
pub fn parse_dpm_levels(text: &str) -> (Option<u32>, Option<u32>) {
    let mut current = None;
    let mut max = None;
    for line in text.lines() {
        let Some((_, rest)) = line.split_once(':') else {
            continue;
        };
        let rest = rest.trim();
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        let Ok(mhz) = digits.parse::<u32>() else {
            continue;
        };
        max = max.max(Some(mhz));
        if rest.ends_with('*') {
            current = Some(mhz);
        }
    }
    (current, max)
}

impl Collector for GpuCollector {
    fn name(&self) -> &'static str {
        "gpu"
    }

    fn collect(&mut self, _root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        let devices = self
            .cards
            .iter()
            .map(|card| {
                let asleep =
                    fs::read_to_string(&card.runtime_status).is_ok_and(|s| s.trim() == "suspended");
                let reading = if asleep {
                    NvmlReading::default()
                } else {
                    match (&self.nvml, &card.nvml) {
                        (Some(lib), Some(device)) => lib.read(device),
                        _ => read_files(&card.files),
                    }
                };
                GpuDevice {
                    id: card.id.clone(),
                    name: card.name.clone(),
                    vendor: card.vendor,
                    driver: card.driver.clone(),
                    asleep,
                    usage: reading.usage,
                    vram_used: reading.vram_used,
                    vram_total: reading.vram_total,
                    freq_mhz: reading.freq_mhz,
                    max_freq_mhz: reading.max_freq_mhz,
                    mem_freq_mhz: reading.mem_freq_mhz,
                    mem_max_freq_mhz: reading.mem_max_freq_mhz,
                    temp_c: reading.temp_c,
                    power_w: reading.power_w,
                    power_cap_w: reading.power_cap_w,
                    fan_rpm: (!asleep && card.nvml.is_none())
                        .then(|| read_num(&card.files.fan))
                        .flatten(),
                    fan_percent: reading.fan_percent,
                }
            })
            .collect();
        out.gpu = Some(GpuSample { devices });
        Ok(())
    }
}

/// One tick of a card read from sysfs.
fn read_files(f: &Files) -> NvmlReading {
    let levels = |p: &Option<PathBuf>| {
        p.as_ref()
            .and_then(|p| fs::read_to_string(p).ok())
            .map(|t| parse_dpm_levels(&t))
            .unwrap_or((None, None))
    };
    let (sclk_now, sclk_max) = levels(&f.sclk_levels);
    let (mclk_now, mclk_max) = levels(&f.mclk_levels);
    let hz_to_mhz = |p: &Option<PathBuf>| read_num::<u64>(p).map(|hz| (hz / 1_000_000) as u32);
    let watts = |p: &Option<PathBuf>| read_num::<u64>(p).map(|uw| uw as f32 / 1_000_000.0);
    NvmlReading {
        usage: read_num::<f32>(&f.busy).map(|pct| (pct / 100.0).clamp(0.0, 1.0)),
        vram_used: read_num(&f.vram_used),
        vram_total: read_num(&f.vram_total).filter(|t| *t > 0),
        // The measured clock is more precise than the level it sits at.
        freq_mhz: hz_to_mhz(&f.sclk_hz)
            .or(sclk_now)
            .or_else(|| read_num(&f.act_mhz)),
        max_freq_mhz: sclk_max.or_else(|| read_num(&f.max_mhz)).filter(|m| *m > 0),
        mem_freq_mhz: hz_to_mhz(&f.mclk_hz).or(mclk_now),
        mem_max_freq_mhz: mclk_max.filter(|m| *m > 0),
        temp_c: read_num::<i64>(&f.temp).and_then(|mc| u32::try_from(mc / 1000).ok()),
        power_w: watts(&f.power),
        power_cap_w: watts(&f.power_cap).filter(|w| *w > 0.0),
        fan_percent: None,
    }
}

fn read_num<T: std::str::FromStr>(path: &Option<PathBuf>) -> Option<T> {
    fs::read_to_string(path.as_ref()?).ok()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dpm_levels_give_current_and_highest_clock() {
        let text = "0: 300Mhz\n1: 600Mhz\n2: 1366Mhz *\n3: 1411Mhz\n";
        assert_eq!(parse_dpm_levels(text), (Some(1366), Some(1411)));
        assert_eq!(parse_dpm_levels("0: 300MHz *\n"), (Some(300), Some(300)));
        assert_eq!(parse_dpm_levels(""), (None, None));
    }

    #[test]
    fn vendor_is_named_once() {
        assert_eq!(
            with_vendor(GpuVendor::Amd, "Radeon RX 580"),
            "AMD Radeon RX 580"
        );
        assert_eq!(
            with_vendor(GpuVendor::Nvidia, "NVIDIA GeForce RTX 3060"),
            "NVIDIA GeForce RTX 3060"
        );
    }
}
