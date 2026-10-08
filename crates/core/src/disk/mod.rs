//! Disk module: activity and speed per physical disk, space per filesystem.

mod diskstats;
mod mountinfo;

pub use diskstats::{parse_diskstats, DiskStat, IoCounters, SECTOR_BYTES};
pub use mountinfo::{local_filesystems, parse_mountinfo, MountEntry};
