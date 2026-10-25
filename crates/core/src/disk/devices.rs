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

/// The temperature sensor of a disk: the hwmon chip whose `device` is the
/// disk's device. For SATA (drivetemp) that is the disk's SCSI device. For
/// NVMe it is the controller: with native multipath, which most
/// distributions enable, `/sys/block/nvme0n1/device` is the NVMe subsystem,
/// and the controllers (`nvme0`) are links inside it.
fn temp_path(root: &SysRoot, name: &str) -> Option<PathBuf> {
    let device = root.sys_path(&format!("block/{name}/device"));
    let mut candidates: Vec<PathBuf> = device.canonicalize().into_iter().collect();
    if let Ok(entries) = std::fs::read_dir(&device) {
        for entry in entries.flatten() {
            let entry_name = entry.file_name();
            let entry_name = entry_name.to_string_lossy();
            let controller = entry_name
                .strip_prefix("nvme")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
            if controller {
                candidates.extend(entry.path().canonicalize());
            }
        }
    }
    if candidates.is_empty() {
        return None;
    }
    std::fs::read_dir(root.sys_path("class/hwmon"))
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|chip| {
            chip.join("device")
                .canonicalize()
                .is_ok_and(|d| candidates.contains(&d))
        })
        .map(|chip| chip.join("temp1_input"))
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
