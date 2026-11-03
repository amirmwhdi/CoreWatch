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
//! - i915 and xe (Intel): the GT frequency, and the load from the time the
//!   GT spent idle (RC6 residency, `rc6_residency_ms` on i915,
//!   `gtidle/idle_residency_ms` on xe): Intel has no busy counter in sysfs,
//!   and the engine counters (perf) need root.
//! - nouveau: the hwmon chip only (temperature, fan, power).
//! - nvidia (proprietary): nothing in sysfs; NVML ([`nvml`]) has it all.
//!
//! A laptop's discrete GPU sleeps when unused, and reading its files or
//! asking NVML would wake it. A card whose `power/runtime_status` is
//! `suspended` is reported as asleep and not read.
//!
//! NVML also keeps an NVIDIA card awake for as long as it is open. On a
//! laptop (runtime power management on, `power/control` is `auto`) it is
//! therefore opened only while the card is awake anyway, and closed again
//! when the card has been idle for a few seconds, so the card can go back
//! to sleep ([`NvmlPolicy`]). A desktop card keeps NVML open.

mod nvml;
pub mod pci;

use crate::{CollectError, Collector, Snapshot, SysRoot};
use nvml::{Nvml, NvmlDevice, NvmlReading};
use pci::PciIds;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

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
    /// The load comes from a counter and is `None` until its second
    /// reading (Intel), rather than not being reported at all.
    pub usage_pending: bool,
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
    /// Intel: milliseconds the GT has spent idle (in RC6) since boot.
    idle_ms: Option<PathBuf>,
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
    /// Read through NVML (the proprietary NVIDIA driver).
    nvidia: bool,
    runtime_status: PathBuf,
    files: Files,
    nvml: Option<NvmlDevice>,
    /// The card sleeps when unused (`power/control` is `auto`).
    runtime_pm: bool,
    /// The load NVML reported last tick, to know when the card is idle.
    last_nvml_usage: Option<f32>,
    /// The idle counter and when it was read, for the next tick's load.
    last_idle: Option<(u64, Instant)>,
}

pub struct GpuCollector {
    cards: Vec<Card>,
    // Declared after `cards`: their NVML handles go before NVML shuts down.
    nvml: Option<Nvml>,
    nvml_policy: NvmlPolicy,
}

/// How long an NVIDIA laptop card must show no load before NVML is closed
/// so it can sleep.
pub const NVML_IDLE_CLOSE: Duration = Duration::from_secs(5);
/// After closing NVML for an idle card, how long to wait before opening it
/// again while the card stays awake (something else, an external display
/// for one, may be keeping it awake).
pub const NVML_REOPEN_AFTER: Duration = Duration::from_secs(30);

/// When NVML should be open. Pure state, tested on its own.
#[derive(Debug, Default)]
pub struct NvmlPolicy {
    /// Since when every NVIDIA card has shown no load.
    idle_since: Option<Instant>,
    /// No opening before this time.
    closed_until: Option<Instant>,
}

/// What [`NvmlPolicy::step`] asks for.
#[derive(Debug, PartialEq, Eq)]
pub enum NvmlAction {
    Open,
    Close,
    Keep,
}

impl NvmlPolicy {
    /// One tick. `runtime_pm`: a laptop card that sleeps when unused.
    /// `awake`: some NVIDIA card is awake now. `idle`: NVML is open and
    /// every card it reads shows no load.
    pub fn step(
        &mut self,
        now: Instant,
        runtime_pm: bool,
        awake: bool,
        open: bool,
        idle: bool,
    ) -> NvmlAction {
        if !awake {
            self.idle_since = None;
            return if open {
                NvmlAction::Close
            } else {
                NvmlAction::Keep
            };
        }
        if !open {
            let waiting = self.closed_until.is_some_and(|until| now < until);
            return if waiting {
                NvmlAction::Keep
            } else {
                self.closed_until = None;
                NvmlAction::Open
            };
        }
        if !runtime_pm || !idle {
            self.idle_since = None;
            return NvmlAction::Keep;
        }
        let since = *self.idle_since.get_or_insert(now);
        if now.saturating_duration_since(since) >= NVML_IDLE_CLOSE {
            self.idle_since = None;
            self.closed_until = Some(now + NVML_REOPEN_AFTER);
            NvmlAction::Close
        } else {
            NvmlAction::Keep
        }
    }
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
                // NVML is opened on the first tick, and only for a card that
                // is awake; until then the name comes from pci.ids.
                let name = pci::model_name(pci_ids.as_deref().unwrap_or_default(), &ids)
                    .map(|model| with_vendor(vendor, &model))
                    .unwrap_or_else(|| format!("{} Graphics", vendor.as_str()));
                Card {
                    files: find_files(&dir, vendor, driver.as_deref()),
                    nvidia: driver.as_deref() == Some("nvidia"),
                    runtime_pm: fs::read_to_string(dir.join("power/control"))
                        .is_ok_and(|c| c.trim() == "auto"),
                    runtime_status: dir.join("power/runtime_status"),
                    id,
                    name,
                    vendor,
                    driver,
                    nvml: None,
                    last_nvml_usage: None,
                    last_idle: None,
                }
            })
            .collect();
        cards.sort_by(|a, b| {
            (a.vendor == GpuVendor::Intel, &a.id).cmp(&(b.vendor == GpuVendor::Intel, &b.id))
        });
        Ok(Self {
            cards,
            nvml: None,
            nvml_policy: NvmlPolicy::default(),
        })
    }

    /// Open or close NVML for this tick; `asleep` is per card.
    fn manage_nvml(&mut self, asleep: &[bool], now: Instant) {
        let nvidia = || self.cards.iter().zip(asleep).filter(|(c, _)| c.nvidia);
        if nvidia().next().is_none() {
            return;
        }
        let awake = nvidia().any(|(_, asleep)| !asleep);
        let runtime_pm = nvidia().any(|(c, _)| c.runtime_pm);
        let idle = self.nvml.is_some()
            && nvidia().all(|(c, _)| c.last_nvml_usage.is_some_and(|u| u < 0.01));
        let action = self
            .nvml_policy
            .step(now, runtime_pm, awake, self.nvml.is_some(), idle);
        match action {
            NvmlAction::Keep => {}
            NvmlAction::Close => {
                tracing::debug!("closing NVML so the card can sleep");
                // The handles go before NVML shuts down.
                for card in &mut self.cards {
                    card.nvml = None;
                    card.last_nvml_usage = None;
                }
                self.nvml = None;
            }
            NvmlAction::Open => {
                let Some(lib) = Nvml::open() else {
                    // No proprietary library: never ask again.
                    for card in &mut self.cards {
                        card.nvidia = false;
                    }
                    return;
                };
                for card in self.cards.iter_mut().filter(|c| c.nvidia) {
                    card.nvml = lib.device(&card.id);
                    if let Some(name) = card.nvml.as_ref().and_then(|d| lib.name(d)) {
                        card.name = name;
                    }
                }
                self.nvml = Some(lib);
            }
        }
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
            let gt = dir.join("tile0/gt0");
            files.act_mhz = existing(gt.join("freq0/act_freq"));
            files.max_mhz = existing(gt.join("freq0/max_freq"));
            files.idle_ms = existing(gt.join("gtidle/idle_residency_ms"));
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
                // RC6 counts idle time only while it is enabled.
                files.idle_ms = [card.join("gt/gt0"), card.join("power")]
                    .into_iter()
                    .find(|dir| dir.join("rc6_residency_ms").is_file())
                    .filter(|dir| {
                        fs::read_to_string(dir.join("rc6_enable")).map_or(true, |s| s.trim() != "0")
                    })
                    .map(|dir| dir.join("rc6_residency_ms"));
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
        let asleep: Vec<bool> = self
            .cards
            .iter()
            .map(|card| {
                fs::read_to_string(&card.runtime_status).is_ok_and(|s| s.trim() == "suspended")
            })
            .collect();
        self.manage_nvml(&asleep, Instant::now());
        let devices = self
            .cards
            .iter_mut()
            .zip(asleep)
            .map(|(card, asleep)| {
                let mut reading = if asleep {
                    NvmlReading::default()
                } else {
                    match (&self.nvml, &card.nvml) {
                        (Some(lib), Some(device)) => {
                            let reading = lib.read(device);
                            card.last_nvml_usage = reading.usage;
                            reading
                        }
                        // An NVIDIA card while NVML is closed: its files say
                        // nothing, and reading them could keep it awake.
                        _ if card.nvidia => NvmlReading::default(),
                        _ => read_files(&card.files),
                    }
                };
                if reading.usage.is_none() && !asleep {
                    reading.usage = card.idle_load();
                }
                GpuDevice {
                    id: card.id.clone(),
                    name: card.name.clone(),
                    vendor: card.vendor,
                    driver: card.driver.clone(),
                    asleep,
                    usage_pending: reading.usage.is_none() && card.files.idle_ms.is_some(),
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
                    fan_rpm: (!asleep && !card.nvidia)
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

impl Card {
    /// The share of the time since the last tick the GT was not idle.
    fn idle_load(&mut self) -> Option<f32> {
        let idle: u64 = read_num(&self.files.idle_ms)?;
        let now = Instant::now();
        let last = self.last_idle.replace((idle, now))?;
        load_from_idle(idle.checked_sub(last.0)?, now - last.1)
    }
}

/// Load from the idle time that passed during `wall` time. The counters are
/// read at slightly different moments, so the result is clamped.
pub fn load_from_idle(idle_ms: u64, wall: Duration) -> Option<f32> {
    let wall_ms = wall.as_secs_f64() * 1000.0;
    (wall_ms >= 1.0).then(|| (1.0 - idle_ms as f64 / wall_ms).clamp(0.0, 1.0) as f32)
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
    fn nvml_lets_a_laptop_card_sleep() {
        let t0 = Instant::now();
        let at = |secs: u64| t0 + Duration::from_secs(secs);
        let mut p = NvmlPolicy::default();
        // Asleep at start: NVML is not opened.
        assert_eq!(p.step(at(0), true, false, false, false), NvmlAction::Keep);
        // A game wakes the card: open, and keep it open while it works.
        assert_eq!(p.step(at(1), true, true, false, false), NvmlAction::Open);
        assert_eq!(p.step(at(2), true, true, true, false), NvmlAction::Keep);
        // The game ends: idle for a few seconds, then closed.
        assert_eq!(p.step(at(3), true, true, true, true), NvmlAction::Keep);
        assert_eq!(p.step(at(7), true, true, true, true), NvmlAction::Keep);
        assert_eq!(p.step(at(8), true, true, true, true), NvmlAction::Close);
        // Still awake (autosuspend delay): not opened again right away.
        assert_eq!(p.step(at(9), true, true, false, false), NvmlAction::Keep);
        assert_eq!(p.step(at(37), true, true, false, false), NvmlAction::Keep);
        // Something else keeps it awake: open again after the pause.
        assert_eq!(p.step(at(38), true, true, false, false), NvmlAction::Open);
    }

    #[test]
    fn nvml_closes_when_the_card_falls_asleep() {
        let now = Instant::now();
        let mut p = NvmlPolicy::default();
        assert_eq!(p.step(now, true, false, true, false), NvmlAction::Close);
    }

    #[test]
    fn nvml_stays_open_on_a_desktop_card() {
        let t0 = Instant::now();
        let mut p = NvmlPolicy::default();
        assert_eq!(p.step(t0, false, true, false, false), NvmlAction::Open);
        let later = t0 + Duration::from_secs(3600);
        assert_eq!(p.step(later, false, true, true, true), NvmlAction::Keep);
    }

    #[test]
    fn dpm_levels_give_current_and_highest_clock() {
        let text = "0: 300Mhz\n1: 600Mhz\n2: 1366Mhz *\n3: 1411Mhz\n";
        assert_eq!(parse_dpm_levels(text), (Some(1366), Some(1411)));
        assert_eq!(parse_dpm_levels("0: 300MHz *\n"), (Some(300), Some(300)));
        assert_eq!(parse_dpm_levels(""), (None, None));
    }

    #[test]
    fn load_is_the_time_not_spent_idle() {
        let second = Duration::from_secs(1);
        assert_eq!(load_from_idle(750, second), Some(0.25));
        assert_eq!(load_from_idle(0, second), Some(1.0));
        // The idle counter ran a little ahead of the clock.
        assert_eq!(load_from_idle(1010, second), Some(0.0));
        assert_eq!(load_from_idle(0, Duration::ZERO), None);
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
