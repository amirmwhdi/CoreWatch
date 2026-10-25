use crate::{
    battery::BatterySample, cpu::CpuSample, disk::DiskSample, gpu::GpuSample, memory::MemorySample,
    network::NetworkSample, process::ProcessSample, sensors::SensorSample,
};
use std::time::Instant;

/// Everything collected in one tick.
///
/// The snapshot is the only thing that crosses from the sampler thread to the
/// UI. Each module owns exactly one `Option` field; `None` means "not
/// collected, or the collector failed this tick".
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct Snapshot {
    pub taken_at: Instant,
    pub cpu: Option<CpuSample>,
    pub memory: Option<MemorySample>,
    pub gpu: Option<GpuSample>,
    pub disk: Option<DiskSample>,
    pub battery: Option<BatterySample>,
    pub network: Option<NetworkSample>,
    pub processes: Option<ProcessSample>,
    pub sensors: Option<SensorSample>,
    // A new module adds one field here and one line in `new`.
}

impl Snapshot {
    pub fn new() -> Self {
        Self {
            taken_at: Instant::now(),
            cpu: None,
            memory: None,
            disk: None,
            gpu: None,
            battery: None,
            network: None,
            processes: None,
            sensors: None,
        }
    }
}

impl Default for Snapshot {
    fn default() -> Self {
        Self::new()
    }
}
