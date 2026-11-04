//! Memory module: RAM and swap.

mod meminfo;

pub use meminfo::parse_meminfo;

use crate::pressure::{self, Pressure, Resource};
use crate::{CollectError, Collector, Snapshot, SysRoot};

/// RAM and swap in one sample. All sizes in bytes.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct MemorySample {
    pub total: u64,
    pub available: u64,
    pub used: u64,
    pub cached: u64,
    pub buffers: u64,
    pub swap_total: u64,
    pub swap_used: u64,
    /// How much of the time tasks waited for memory (reclaim, swap-in);
    /// `None` without PSI.
    pub pressure: Option<Pressure>,
}

impl MemorySample {
    /// Used RAM as a share of total, 0.0..=1.0.
    pub fn used_fraction(&self) -> f32 {
        fraction(self.used, self.total)
    }

    /// Used swap as a share of total swap; 0.0 when there is no swap.
    pub fn swap_fraction(&self) -> f32 {
        fraction(self.swap_used, self.swap_total)
    }
}

fn fraction(part: u64, whole: u64) -> f32 {
    if whole == 0 {
        0.0
    } else {
        (part as f64 / whole as f64).clamp(0.0, 1.0) as f32
    }
}

/// Stateless: every sample is a fresh read of `/proc/meminfo`.
#[derive(Debug, Default)]
pub struct MemoryCollector;

impl Collector for MemoryCollector {
    fn name(&self) -> &'static str {
        "memory"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        let mut sample = parse_meminfo(&root.read_proc("meminfo")?)?;
        sample.pressure = pressure::read(root, Resource::Memory);
        out.memory = Some(sample);
        Ok(())
    }
}
