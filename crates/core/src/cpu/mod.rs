//! CPU module: model, total and per-core usage, frequency.

mod info;
mod stat;

pub use info::CpuInfo;
pub use stat::{parse_stat, CoreTimes, CpuTimes, StatTimes};

use crate::{CollectError, Collector, Snapshot, SysRoot};
use std::sync::Arc;

/// One logical core in one sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CoreSample {
    /// Kernel core number (`cpuN`).
    pub id: usize,
    /// Busy share since the previous sample, 0.0..=1.0.
    pub usage: f32,
    /// Current frequency, if the kernel exposes it.
    pub freq_mhz: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct CpuSample {
    pub info: Arc<CpuInfo>,
    /// Whole CPU, 0.0..=1.0.
    pub usage: f32,
    /// One entry per online logical core, in kernel order.
    pub cores: Vec<CoreSample>,
}

impl CpuSample {
    /// Highest current core frequency, if any core reports one.
    pub fn max_freq_mhz(&self) -> Option<u32> {
        self.cores.iter().filter_map(|c| c.freq_mhz).max()
    }
}

pub struct CpuCollector {
    info: Arc<CpuInfo>,
    prev: StatTimes,
}

impl CpuCollector {
    /// Reads the static info and primes the counters, so the first sample
    /// already holds real usage.
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        let info = Arc::new(info::parse_cpuinfo(&root.read_proc("cpuinfo")?));
        let prev = stat::parse_stat(&root.read_proc("stat")?)?;
        Ok(Self { info, prev })
    }
}

impl Collector for CpuCollector {
    fn name(&self) -> &'static str {
        "cpu"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        let now = stat::parse_stat(&root.read_proc("stat")?)?;

        let cores = now
            .cores
            .iter()
            .map(|core| {
                // Match by kernel id, so a core going offline does not shift the rest.
                let usage = self
                    .prev
                    .cores
                    .iter()
                    .find(|p| p.id == core.id)
                    .map_or(0.0, |p| core.times.usage_since(&p.times));
                CoreSample {
                    id: core.id,
                    usage,
                    freq_mhz: info::read_freq_mhz(root, core.id),
                }
            })
            .collect();

        out.cpu = Some(CpuSample {
            info: Arc::clone(&self.info),
            usage: now.total.usage_since(&self.prev.total),
            cores,
        });
        self.prev = now;
        Ok(())
    }
}
