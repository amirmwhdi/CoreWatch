//! Drive SMART data from UDisks: health for every drive it knows, and the
//! temperature of drives the kernel gives no sensor.
//!
//! The kernel shows a SATA drive's temperature only when the `drivetemp`
//! module is loaded, which most distributions do not do, and it shows no
//! drive health at all. UDisks reads the drive's SMART data itself (as root,
//! about every ten minutes) and offers it on the system bus to any user, so
//! Corewatch asks it once a minute. Hard disks and SATA SSDs report through
//! UDisks' `Drive.Ata` interface, NVMe drives through `NVMe.Controller`. The
//! Disks page prefers the kernel's temperature sensor when there is one.

use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

/// UDisks refreshes SMART data every ten minutes; once a minute is plenty.
const REFRESH: Duration = Duration::from_secs(60);

/// What SMART says about one drive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriveSmart {
    /// °C; `None` when the drive reports none.
    pub temp_c: Option<f32>,
    /// `None` when SMART is unsupported, off, or not read yet.
    pub health: Option<Health>,
}

/// A drive's SMART health. Fields a drive does not report are `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    /// The drive predicts its own failure (ATA: an attribute below its
    /// threshold flagged as pre-fail, or the overall assessment failed).
    pub failing: bool,
    /// ATA attributes below their threshold right now.
    pub attributes_failing: Option<u32>,
    /// ATA reallocated plus pending sectors.
    pub bad_sectors: Option<u64>,
    /// NVMe critical warnings, e.g. `"spare"`, `"readonly"`.
    pub critical_warnings: Vec<String>,
    pub power_on_hours: Option<u64>,
    /// The last self-test ended in an error.
    pub selftest_failed: bool,
}

impl Health {
    /// Anything worth the user's attention.
    pub fn is_good(&self) -> bool {
        !self.failing
            && !self.selftest_failed
            && self.critical_warnings.is_empty()
            && self.attributes_failing.unwrap_or(0) == 0
            && self.bad_sectors.unwrap_or(0) == 0
    }
}

thread_local! {
    /// SMART data by kernel device name (`"sda"`).
    static DRIVES: RefCell<HashMap<String, DriveSmart>> = RefCell::default();
}

/// The latest UDisks temperature of `/dev/<name>`.
pub fn celsius(name: &str) -> Option<f32> {
    DRIVES.with(|d| d.borrow().get(name).and_then(|s| s.temp_c))
}

/// The latest SMART health of `/dev/<name>`.
pub fn health(name: &str) -> Option<Health> {
    DRIVES.with(|d| d.borrow().get(name).and_then(|s| s.health.clone()))
}

/// After a failed call, wait this long and double it, up to [`REFRESH`]
/// times ten: UDisks may start after Corewatch (early in a session), or be
/// restarted.
const RETRY_FIRST: Duration = Duration::from_secs(10);

/// Ask UDisks now and then every [`REFRESH`]. When it does not answer
/// (not running yet, not installed, or no permission in Flatpak), ask again
/// later, less and less often; a missing UDisks costs one D-Bus call every
/// ten minutes.
pub fn start() {
    glib::spawn_future_local(async {
        let bus = match gio::bus_get_future(gio::BusType::System).await {
            Ok(bus) => bus,
            Err(e) => return tracing::debug!("no system bus for drive SMART data: {e}"),
        };
        let mut retry = RETRY_FIRST;
        let mut failing = false;
        loop {
            let wait = match managed_objects(&bus).await {
                Ok(objects) => {
                    if std::mem::take(&mut failing) {
                        tracing::info!("UDisks answers again");
                    }
                    let drives = parse_objects(&objects);
                    DRIVES.with(|d| *d.borrow_mut() = drives);
                    retry = RETRY_FIRST;
                    REFRESH
                }
                Err(e) => {
                    // No answer: show nothing rather than old data.
                    DRIVES.with(|d| d.borrow_mut().clear());
                    if !std::mem::replace(&mut failing, true) {
                        tracing::debug!("UDisks unavailable, will ask again: {e}");
                    }
                    let wait = retry;
                    retry = (retry * 2).min(REFRESH * 10);
                    wait
                }
            };
            glib::timeout_future(wait).await;
        }
    });
}

async fn managed_objects(bus: &gio::DBusConnection) -> Result<glib::Variant, glib::Error> {
    let reply = bus
        .call_future(
            Some("org.freedesktop.UDisks2"),
            "/org/freedesktop/UDisks2",
            "org.freedesktop.DBus.ObjectManager",
            "GetManagedObjects",
            None,
            None,
            gio::DBusCallFlags::NONE,
            5000,
        )
        .await?;
    Ok(reply.child_value(0))
}

fn lookup<T: glib::variant::FromVariant>(props: &glib::VariantDict, key: &str) -> Option<T> {
    props.lookup_value(key, None).and_then(|v| v.get::<T>())
}

/// Self-test results that mean the drive found a fault. ATA reports
/// `"error_read"`, `"error_electrical"` and the like; NVMe `"fatal_error"`,
/// `"known_seg_fail"`, `"unknown_seg_fail"`. Aborted or interrupted tests
/// say nothing about the drive.
fn selftest_failed(status: &str) -> bool {
    status.starts_with("error") || status.starts_with("fatal") || status.ends_with("_fail")
}

/// SMART data of an ATA drive (hard disk or SATA SSD) from its
/// `Drive.Ata` properties.
fn parse_ata(props: &glib::VariantDict) -> DriveSmart {
    // Kelvin, 0 when unknown.
    let temp_c = lookup::<f64>(props, "SmartTemperature")
        .filter(|k| *k > 0.0)
        .map(|k| (k - 273.15) as f32);
    let read = lookup::<bool>(props, "SmartSupported").unwrap_or(false)
        && lookup::<bool>(props, "SmartEnabled").unwrap_or(false)
        && lookup::<u64>(props, "SmartUpdated").unwrap_or(0) > 0;
    let health = read.then(|| Health {
        failing: lookup::<bool>(props, "SmartFailing").unwrap_or(false),
        // -1 when unknown.
        attributes_failing: lookup::<i32>(props, "SmartNumAttributesFailing")
            .and_then(|n| u32::try_from(n).ok()),
        bad_sectors: lookup::<i64>(props, "SmartNumBadSectors").and_then(|n| u64::try_from(n).ok()),
        critical_warnings: Vec::new(),
        power_on_hours: lookup::<u64>(props, "SmartPowerOnSeconds")
            .filter(|s| *s > 0)
            .map(|s| s / 3600),
        selftest_failed: lookup::<String>(props, "SmartSelftestStatus")
            .is_some_and(|s| selftest_failed(&s)),
    });
    DriveSmart { temp_c, health }
}

/// SMART data of an NVMe drive from its `NVMe.Controller` properties. The
/// temperature is left out: the kernel's `nvme` sensor always has it.
fn parse_nvme(props: &glib::VariantDict) -> DriveSmart {
    let read = lookup::<u64>(props, "SmartUpdated").unwrap_or(0) > 0;
    let health = read.then(|| Health {
        critical_warnings: lookup::<Vec<String>>(props, "SmartCriticalWarning").unwrap_or_default(),
        power_on_hours: lookup::<u64>(props, "SmartPowerOnHours"),
        selftest_failed: lookup::<String>(props, "SmartSelftestStatus")
            .is_some_and(|s| selftest_failed(&s)),
        ..Health::default()
    });
    DriveSmart {
        temp_c: None,
        health,
    }
}

/// SMART data by device name from a `GetManagedObjects` reply
/// (`a{oa{sa{sv}}}`): each Block object names its device and its Drive, and
/// the Drive object's `Drive.Ata` or `NVMe.Controller` interface holds the
/// SMART data.
pub fn parse_objects(objects: &glib::Variant) -> HashMap<String, DriveSmart> {
    let mut drives: HashMap<String, DriveSmart> = HashMap::new();
    let mut blocks: Vec<(String, String)> = Vec::new();
    for object in objects.iter() {
        let path = object.child_value(0);
        let Some(path) = path.str() else { continue };
        for interface in object.child_value(1).iter() {
            let name = interface.child_value(0);
            let props = glib::VariantDict::new(Some(&interface.child_value(1)));
            match name.str() {
                Some("org.freedesktop.UDisks2.Drive.Ata") => {
                    drives.insert(path.to_owned(), parse_ata(&props));
                }
                Some("org.freedesktop.UDisks2.NVMe.Controller") => {
                    drives.insert(path.to_owned(), parse_nvme(&props));
                }
                Some("org.freedesktop.UDisks2.Block") => {
                    let device = lookup::<Vec<u8>>(&props, "Device").map(|bytes| {
                        let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
                        String::from_utf8_lossy(&bytes[..end]).into_owned()
                    });
                    let drive = props
                        .lookup_value("Drive", None)
                        .and_then(|v| v.str().map(str::to_owned));
                    if let (Some(device), Some(drive)) = (device, drive) {
                        if let Some(name) = device.strip_prefix("/dev/") {
                            blocks.push((name.to_owned(), drive));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    blocks
        .into_iter()
        .filter_map(|(name, drive)| Some((name, drives.get(&drive)?.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    type Props = HashMap<String, glib::Variant>;

    /// A `GetManagedObjects` reply with one drive (`D1`) holding `iface`,
    /// and the block devices `sda` and `sda1` on it.
    fn objects(iface: &str, drive_props: Props) -> glib::Variant {
        let mut drive = HashMap::new();
        drive.insert(iface.to_owned(), drive_props);

        let block = |dev: &str| {
            let mut props = Props::new();
            let mut bytes = dev.as_bytes().to_vec();
            bytes.push(0);
            props.insert("Device".into(), bytes.to_variant());
            props.insert(
                "Drive".into(),
                glib::variant::ObjectPath::try_from(
                    "/org/freedesktop/UDisks2/drives/D1".to_owned(),
                )
                .unwrap()
                .to_variant(),
            );
            let mut ifaces = HashMap::new();
            ifaces.insert("org.freedesktop.UDisks2.Block".to_owned(), props);
            ifaces
        };
        let mut objects = HashMap::new();
        objects.insert("/org/freedesktop/UDisks2/drives/D1".to_owned(), drive);
        objects.insert(
            "/org/freedesktop/UDisks2/block_devices/sda".to_owned(),
            block("/dev/sda"),
        );
        objects.insert(
            "/org/freedesktop/UDisks2/block_devices/sda1".to_owned(),
            block("/dev/sda1"),
        );
        objects.to_variant()
    }

    fn hard_disk() -> Props {
        let mut ata = Props::new();
        ata.insert("SmartSupported".into(), true.to_variant());
        ata.insert("SmartEnabled".into(), true.to_variant());
        ata.insert("SmartUpdated".into(), 1_700_000_000u64.to_variant());
        ata.insert("SmartFailing".into(), false.to_variant());
        ata.insert("SmartTemperature".into(), 310.15f64.to_variant());
        ata.insert("SmartPowerOnSeconds".into(), (3600u64 * 9000).to_variant());
        ata.insert("SmartNumAttributesFailing".into(), 0i32.to_variant());
        ata.insert("SmartNumBadSectors".into(), 0i64.to_variant());
        ata.insert("SmartSelftestStatus".into(), "success".to_variant());
        ata
    }

    #[test]
    fn blocks_take_their_drive_temperature() {
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.Drive.Ata", hard_disk()));
        assert!((drives["sda"].temp_c.unwrap() - 37.0).abs() < 0.01);
        assert!(drives.contains_key("sda1"));
    }

    #[test]
    fn healthy_hard_disk() {
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.Drive.Ata", hard_disk()));
        let health = drives["sda"].health.clone().unwrap();
        assert!(health.is_good());
        assert_eq!(health.power_on_hours, Some(9000));
        assert_eq!(health.bad_sectors, Some(0));
    }

    #[test]
    fn hard_disk_with_bad_sectors_and_failed_selftest() {
        let mut ata = hard_disk();
        ata.insert("SmartNumBadSectors".into(), 8i64.to_variant());
        ata.insert("SmartSelftestStatus".into(), "error_read".to_variant());
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.Drive.Ata", ata));
        let health = drives["sda"].health.clone().unwrap();
        assert!(!health.is_good());
        assert!(!health.failing);
        assert_eq!(health.bad_sectors, Some(8));
        assert!(health.selftest_failed);
    }

    #[test]
    fn unknown_counts_are_none() {
        let mut ata = hard_disk();
        ata.insert("SmartNumBadSectors".into(), (-1i64).to_variant());
        ata.insert("SmartNumAttributesFailing".into(), (-1i32).to_variant());
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.Drive.Ata", ata));
        let health = drives["sda"].health.clone().unwrap();
        assert_eq!(health.bad_sectors, None);
        assert_eq!(health.attributes_failing, None);
        assert!(health.is_good());
    }

    #[test]
    fn no_health_when_smart_is_off_or_unread() {
        let mut off = hard_disk();
        off.insert("SmartEnabled".into(), false.to_variant());
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.Drive.Ata", off));
        assert_eq!(drives["sda"].health, None);
        // The temperature still counts.
        assert!(drives["sda"].temp_c.is_some());

        let mut unread = hard_disk();
        unread.insert("SmartUpdated".into(), 0u64.to_variant());
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.Drive.Ata", unread));
        assert_eq!(drives["sda"].health, None);
    }

    #[test]
    fn nvme_critical_warnings() {
        let mut nvme = Props::new();
        nvme.insert("SmartUpdated".into(), 1_700_000_000u64.to_variant());
        nvme.insert("SmartPowerOnHours".into(), 2248u64.to_variant());
        nvme.insert(
            "SmartCriticalWarning".into(),
            vec!["spare".to_owned()].to_variant(),
        );
        nvme.insert("SmartTemperature".into(), 300u16.to_variant());
        let drives = parse_objects(&objects("org.freedesktop.UDisks2.NVMe.Controller", nvme));
        let smart = &drives["sda"];
        assert_eq!(smart.temp_c, None);
        let health = smart.health.clone().unwrap();
        assert_eq!(health.power_on_hours, Some(2248));
        assert_eq!(health.critical_warnings, ["spare"]);
        assert!(!health.is_good());
    }

    #[test]
    fn selftest_results() {
        for bad in [
            "error_read",
            "error_unknown",
            "fatal_error",
            "known_seg_fail",
        ] {
            assert!(selftest_failed(bad), "{bad}");
        }
        for fine in ["success", "aborted", "interrupted", "inprogress", ""] {
            assert!(!selftest_failed(fine), "{fine}");
        }
    }
}
