//! Which block devices are whole physical disks, and what they are.

use super::DiskKind;
use crate::SysRoot;
use std::path::PathBuf;

/// Software devices that stack on real disks or live in RAM. Counting them
/// would show the same I/O twice. They also lack a `device` link, which is the
/// main test below; the prefix check just skips them without touching `/sys`.
const VIRTUAL_PREFIXES: [&str; 5] = ["loop", "ram", "zram", "dm-", "md"];

/// `/sys/block/<name>/size` is always in 512-byte units.
const SIZE_UNIT: u64 = 512;

/// Size in bytes when `name` is a whole physical disk with media inside;
/// `None` for partitions, virtual devices and empty card readers.
pub(crate) fn physical_size(root: &SysRoot, name: &str) -> Option<u64> {
    if name.contains('/') || VIRTUAL_PREFIXES.iter().any(|p| name.starts_with(p)) {
        return None;
    }
    // Partitions have no /sys/block entry; virtual disks have no `device`.
    if !root.sys_path(&format!("block/{name}/device")).exists() {
        return None;
    }
    let sectors: u64 = root
        .read_sys(&format!("block/{name}/size"))
        .ok()?
        .trim()
        .parse()
        .ok()?;
    (sectors > 0).then(|| sectors.saturating_mul(SIZE_UNIT))
}

/// Facts about a disk that do not change while it is plugged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DeviceInfo {
    pub model: String,
    pub kind: DiskKind,
    /// The drive's own temperature sensor (`temp1_input` of its hwmon chip:
    /// "Composite" on NVMe, the drivetemp sensor on SATA).
    pub temp_path: Option<PathBuf>,
}

/// The hwmon chip of a disk sits under its device, either directly
/// (`device/hwmon3`) or in a `hwmon` folder (`device/hwmon/hwmon3`).
fn temp_path(root: &SysRoot, name: &str) -> Option<PathBuf> {
    let device = root.sys_path(&format!("block/{name}/device"));
    [device.clone(), device.join("hwmon")]
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with("hwmon"))
        .map(|e| e.path().join("temp1_input"))
        .find(|p| p.is_file())
}

pub(crate) fn read_info(root: &SysRoot, name: &str) -> DeviceInfo {
    let read = |rel: &str| {
        root.read_sys(&format!("block/{name}/{rel}"))
            .ok()
            .map(|text| text.split_whitespace().collect::<Vec<_>>().join(" "))
            .filter(|text| !text.is_empty())
    };

    // SATA, SCSI, USB and NVMe disks have `model`; SD and eMMC cards have `name`.
    let model = read("device/model")
        .or_else(|| read("device/name"))
        .unwrap_or_else(|| name.to_owned());

    let kind = if name.starts_with("nvme") {
        DiskKind::Nvme
    } else if read("removable").as_deref() == Some("1") {
        DiskKind::Removable
    } else if read("queue/rotational").as_deref() == Some("1") {
        DiskKind::Hdd
    } else {
        DiskKind::Ssd
    };

    DeviceInfo {
        model,
        kind,
        temp_path: temp_path(root, name),
    }
}
