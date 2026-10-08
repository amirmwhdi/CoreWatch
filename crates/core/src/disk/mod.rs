//! Disk module: activity and speed per physical disk, space per filesystem.

mod devices;
mod diskstats;
mod mountinfo;
mod space;

pub use diskstats::{parse_diskstats, DiskStat, IoCounters, SECTOR_BYTES};
pub use mountinfo::{local_filesystems, parse_mountinfo, MountEntry};
pub use space::{Space, SpaceProbe, Statvfs};

use crate::{CollectError, Collector, Snapshot, SysRoot};
use devices::DeviceInfo;
use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

/// Filesystem space changes slowly; read it once every this many ticks.
pub const FS_EVERY: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiskKind {
    Nvme,
    Ssd,
    Hdd,
    Removable,
}

/// One physical disk in one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct DiskDevice {
    /// Kernel name, e.g. `"nvme0n1"`.
    pub name: String,
    /// Trimmed model string; the kernel name when the disk reports none.
    pub model: String,
    pub kind: DiskKind,
    /// Capacity in bytes.
    pub size: u64,
    /// Bytes per second since the previous sample; `None` on the first
    /// sample after the disk appeared.
    pub read_bps: Option<u64>,
    pub write_bps: Option<u64>,
    /// Share of time the disk was busy, 0.0..=1.0; `None` like the speeds.
    pub busy: Option<f32>,
    /// Bytes read and written since boot.
    pub read_total: u64,
    pub written_total: u64,
}

/// One mounted local filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filesystem {
    pub mount_point: PathBuf,
    /// Device it lives on, e.g. `"/dev/nvme0n1p2"`.
    pub source: String,
    pub fs_type: String,
    pub space: Space,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiskSample {
    /// Sorted by kernel name.
    pub disks: Vec<DiskDevice>,
    /// Sorted by mount point.
    pub filesystems: Vec<Filesystem>,
}

impl DiskSample {
    /// Read plus write speed of every disk together, in bytes per second.
    pub fn total_bps(&self) -> u64 {
        self.disks
            .iter()
            .map(|d| {
                d.read_bps
                    .unwrap_or(0)
                    .saturating_add(d.write_bps.unwrap_or(0))
            })
            .fold(0, u64::saturating_add)
    }
}

pub struct DiskCollector {
    probe: Box<dyn SpaceProbe>,
    prev: HashMap<String, IoCounters>,
    prev_at: Instant,
    info: HashMap<String, DeviceInfo>,
    filesystems: Vec<Filesystem>,
    /// Ticks left until the next filesystem refresh; 0 = refresh now.
    fs_countdown: u32,
    mountinfo_failed: bool,
}

impl DiskCollector {
    /// Reads `diskstats` once to prime the counters, so the first sample
    /// already holds real speeds. Fails only when `diskstats` is unreadable.
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        Self::with_probe(root, Box::new(Statvfs), Instant::now())
    }

    /// Like [`DiskCollector::new`] with a custom space probe and start time;
    /// used by tests.
    pub fn with_probe(
        root: &SysRoot,
        probe: Box<dyn SpaceProbe>,
        now: Instant,
    ) -> Result<Self, CollectError> {
        let prev = parse_diskstats(&root.read_proc("diskstats")?)?
            .into_iter()
            .map(|s| (s.name, s.counters))
            .collect();
        Ok(Self {
            probe,
            prev,
            prev_at: now,
            info: HashMap::new(),
            filesystems: Vec::new(),
            fs_countdown: 0,
            mountinfo_failed: false,
        })
    }

    /// [`Collector::collect`] with an explicit clock; used by tests.
    pub fn collect_at(
        &mut self,
        root: &SysRoot,
        out: &mut Snapshot,
        now: Instant,
    ) -> Result<(), CollectError> {
        let stats = parse_diskstats(&root.read_proc("diskstats")?)?;
        let secs = now.saturating_duration_since(self.prev_at).as_secs_f64();

        let mut disks = Vec::new();
        let mut seen = HashMap::new();
        for DiskStat { name, counters } in stats {
            let Some(size) = devices::physical_size(root, &name) else {
                continue;
            };
            let info = self
                .info
                .entry(name.clone())
                .or_insert_with(|| devices::read_info(root, &name));
            let rates = self
                .prev
                .get(&name)
                .filter(|_| secs > 0.0)
                .map(|before| Rates::between(before, &counters, secs));

            disks.push(DiskDevice {
                model: info.model.clone(),
                kind: info.kind,
                size,
                read_bps: rates.map(|r| r.read_bps),
                write_bps: rates.map(|r| r.write_bps),
                busy: rates.map(|r| r.busy),
                read_total: counters.sectors_read.saturating_mul(SECTOR_BYTES),
                written_total: counters.sectors_written.saturating_mul(SECTOR_BYTES),
                name: name.clone(),
            });
            seen.insert(name, counters);
        }
        disks.sort_by(|a, b| a.name.cmp(&b.name));

        // Forget unplugged disks, so a new disk reusing the name is read afresh.
        self.info.retain(|name, _| seen.contains_key(name));
        self.prev = seen;
        self.prev_at = now;

        if self.fs_countdown == 0 {
            self.refresh_filesystems(root);
            self.fs_countdown = FS_EVERY.saturating_sub(1);
        } else {
            self.fs_countdown -= 1;
        }

        out.disk = Some(DiskSample {
            disks,
            filesystems: self.filesystems.clone(),
        });
        Ok(())
    }

    /// A broken `mountinfo` keeps the last list and is logged once, so disk
    /// activity keeps working.
    fn refresh_filesystems(&mut self, root: &SysRoot) {
        let entries = match root
            .read_proc("self/mountinfo")
            .and_then(|text| parse_mountinfo(&text))
        {
            Ok(entries) => entries,
            Err(error) => {
                if !self.mountinfo_failed {
                    tracing::warn!(
                        error.kind = error.kind(),
                        error = %error.chain(),
                        "filesystem list unavailable"
                    );
                    self.mountinfo_failed = true;
                }
                return;
            }
        };
        self.mountinfo_failed = false;
        self.filesystems = local_filesystems(&entries)
            .into_iter()
            .filter_map(|entry| {
                let space = self.probe.space(&entry.mount_point)?;
                (space.total > 0).then_some(Filesystem {
                    mount_point: entry.mount_point,
                    source: entry.source,
                    fs_type: entry.fs_type,
                    space,
                })
            })
            .collect();
    }
}

impl Collector for DiskCollector {
    fn name(&self) -> &'static str {
        "disk"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        self.collect_at(root, out, Instant::now())
    }
}

#[derive(Debug, Clone, Copy)]
struct Rates {
    read_bps: u64,
    write_bps: u64,
    busy: f32,
}

impl Rates {
    /// A counter that went down (wrapped, or the disk was swapped) counts as
    /// no activity for this tick instead of a huge jump.
    fn between(before: &IoCounters, now: &IoCounters, secs: f64) -> Self {
        let per_sec =
            |sectors: u64| (sectors.saturating_mul(SECTOR_BYTES) as f64 / secs).round() as u64;
        let io_ms = now.io_ms.saturating_sub(before.io_ms) as f64;
        Self {
            read_bps: per_sec(now.sectors_read.saturating_sub(before.sectors_read)),
            write_bps: per_sec(now.sectors_written.saturating_sub(before.sectors_written)),
            busy: (io_ms / (secs * 1000.0)).clamp(0.0, 1.0) as f32,
        }
    }
}
