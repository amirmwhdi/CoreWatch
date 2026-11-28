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
//!
//! The summary (failing, bad sectors, power-on time) is in the interface's
//! properties. The rest (wear, data written, power cycles, each ATA
//! attribute) takes a `SmartGetAttributes` call per drive, made only when
//! UDisks has read the drive again; ATA drives are asked not to wake up, so
//! a sleeping hard disk keeps sleeping.

use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

/// UDisks refreshes SMART data every ten minutes; once a minute is plenty.
const REFRESH: Duration = Duration::from_secs(60);

const ATA: &str = "org.freedesktop.UDisks2.Drive.Ata";
const NVME: &str = "org.freedesktop.UDisks2.NVMe.Controller";

/// What SMART says about one drive.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DriveSmart {
    /// °C; `None` when the drive reports none.
    pub temp_c: Option<f32>,
    /// `None` when SMART is unsupported, off, or not read yet.
    pub health: Option<Health>,
}

/// How a drive reports SMART.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Protocol {
    /// Hard disks and SATA SSDs.
    #[default]
    Ata,
    Nvme,
}

/// A drive's SMART health. Fields a drive does not report are `None`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Health {
    pub protocol: Protocol,
    /// A hard disk (it spins); `None` when UDisks does not know.
    pub rotational: Option<bool>,
    /// When UDisks last read the drive, in seconds since 1970.
    pub updated: u64,
    /// The drive predicts its own failure (ATA: an attribute below its
    /// threshold flagged as pre-fail, or the overall assessment failed).
    pub failing: bool,
    /// ATA attributes below their threshold right now.
    pub attributes_failing: Option<u32>,
    /// ATA attributes that were below their threshold once.
    pub attributes_failed_in_past: Option<u32>,
    /// ATA reallocated plus pending sectors.
    pub bad_sectors: Option<u64>,
    pub reallocated_sectors: Option<u64>,
    pub pending_sectors: Option<u64>,
    pub uncorrectable_sectors: Option<u64>,
    /// NVMe critical warnings, e.g. `"spare"`, `"readonly"`.
    pub critical_warnings: Vec<String>,
    pub power_on_hours: Option<u64>,
    pub power_cycles: Option<u64>,
    /// Power lost without the drive being told first.
    pub unsafe_shutdowns: Option<u64>,
    /// NVMe: unrecovered data integrity errors.
    pub media_errors: Option<u64>,
    /// NVMe: entries in the drive's error log.
    pub error_log_entries: Option<u64>,
    pub data_read: Option<u64>,
    pub data_written: Option<u64>,
    /// NVMe spare capacity left, and the level below which it warns (0–100).
    pub available_spare: Option<u8>,
    pub spare_threshold: Option<u8>,
    /// NVMe: minutes spent above the warning temperature.
    pub minutes_too_hot: Option<u64>,
    /// UDisks' name of the last self-test result, e.g. `"success"`.
    pub selftest: Option<String>,
    /// The last self-test ended in an error.
    pub selftest_failed: bool,
    /// Health, 0–100. On NVMe drives and SSDs with a wear attribute it is
    /// what the drive says of its life left; otherwise see `estimated`.
    pub percent: Option<u8>,
    /// `percent` is Corewatch's estimate from bad sectors and the drive's
    /// reliability attributes, since hard disks report no life left.
    pub estimated: bool,
}

impl Health {
    /// Nothing worth the user's attention.
    pub fn is_good(&self) -> bool {
        !self.failing
            && !self.selftest_failed
            && self.critical_warnings.is_empty()
            && self.attributes_failing.unwrap_or(0) == 0
            && self.bad_sectors.unwrap_or(0) == 0
            && self.media_errors.unwrap_or(0) == 0
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
        // `SmartGetAttributes` replies by drive path, with the `updated`
        // time they belong to.
        let mut attributes: HashMap<String, (u64, glib::Variant)> = HashMap::new();
        loop {
            let wait = match managed_objects(&bus).await {
                Ok(objects) => {
                    if std::mem::take(&mut failing) {
                        tracing::info!("UDisks answers again");
                    }
                    let mut drives = parse_objects(&objects);
                    attributes.retain(|path, _| drives.by_path.contains_key(path));
                    for (path, smart) in &mut drives.by_path {
                        let Some(health) = &mut smart.health else {
                            continue;
                        };
                        let fresh = attributes
                            .get(path)
                            .is_some_and(|(updated, _)| *updated == health.updated);
                        if !fresh {
                            match smart_attributes(&bus, path, health.protocol).await {
                                Ok(reply) => {
                                    attributes.insert(path.clone(), (health.updated, reply));
                                }
                                // Keep the last reply: a sleeping disk is
                                // not asked to wake, and nothing changed.
                                Err(e) => tracing::debug!("no SMART attributes for {path}: {e}"),
                            }
                        }
                        if let Some((_, reply)) = attributes.get(path) {
                            apply_attributes(health, reply);
                        }
                        finish(health);
                    }
                    DRIVES.with(|d| *d.borrow_mut() = drives.by_device());
                    retry = RETRY_FIRST;
                    REFRESH
                }
                Err(e) => {
                    // No answer: show nothing rather than old data.
                    DRIVES.with(|d| d.borrow_mut().clear());
                    attributes.clear();
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

/// `SmartGetAttributes` of one drive: `a(ysqiiixia{sv})` for ATA, `a{sv}`
/// for NVMe.
async fn smart_attributes(
    bus: &gio::DBusConnection,
    path: &str,
    protocol: Protocol,
) -> Result<glib::Variant, glib::Error> {
    let options = glib::VariantDict::new(None);
    let interface = match protocol {
        Protocol::Ata => {
            options.insert("nowakeup", true);
            ATA
        }
        Protocol::Nvme => NVME,
    };
    let reply = bus
        .call_future(
            Some("org.freedesktop.UDisks2"),
            path,
            interface,
            "SmartGetAttributes",
            Some(&(options.end(),).into()),
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
    let updated = lookup::<u64>(props, "SmartUpdated").unwrap_or(0);
    let read = lookup::<bool>(props, "SmartSupported").unwrap_or(false)
        && lookup::<bool>(props, "SmartEnabled").unwrap_or(false)
        && updated > 0;
    let selftest = lookup::<String>(props, "SmartSelftestStatus").filter(|s| !s.is_empty());
    let health = read.then(|| Health {
        protocol: Protocol::Ata,
        updated,
        failing: lookup::<bool>(props, "SmartFailing").unwrap_or(false),
        // -1 when unknown.
        attributes_failing: lookup::<i32>(props, "SmartNumAttributesFailing")
            .and_then(|n| u32::try_from(n).ok()),
        attributes_failed_in_past: lookup::<i32>(props, "SmartNumAttributesFailedInThePast")
            .and_then(|n| u32::try_from(n).ok()),
        bad_sectors: lookup::<i64>(props, "SmartNumBadSectors").and_then(|n| u64::try_from(n).ok()),
        power_on_hours: lookup::<u64>(props, "SmartPowerOnSeconds")
            .filter(|s| *s > 0)
            .map(|s| s / 3600),
        selftest_failed: selftest.as_deref().is_some_and(selftest_failed),
        selftest,
        ..Health::default()
    });
    DriveSmart { temp_c, health }
}

/// SMART data of an NVMe drive from its `NVMe.Controller` properties. The
/// temperature is left out: the kernel's `nvme` sensor always has it.
fn parse_nvme(props: &glib::VariantDict) -> DriveSmart {
    let updated = lookup::<u64>(props, "SmartUpdated").unwrap_or(0);
    let selftest = lookup::<String>(props, "SmartSelftestStatus").filter(|s| !s.is_empty());
    let health = (updated > 0).then(|| Health {
        protocol: Protocol::Nvme,
        rotational: Some(false),
        updated,
        critical_warnings: lookup::<Vec<String>>(props, "SmartCriticalWarning").unwrap_or_default(),
        power_on_hours: lookup::<u64>(props, "SmartPowerOnHours"),
        selftest_failed: selftest.as_deref().is_some_and(selftest_failed),
        selftest,
        ..Health::default()
    });
    DriveSmart {
        temp_c: None,
        health,
    }
}

/// Add what a `SmartGetAttributes` reply holds.
fn apply_attributes(health: &mut Health, reply: &glib::Variant) {
    match health.protocol {
        Protocol::Nvme => apply_nvme(health, reply),
        Protocol::Ata => apply_ata(health, reply),
    }
}

/// NVMe attributes (`a{sv}`): the drive's own wear estimate, spare
/// capacity, and counters.
fn apply_nvme(health: &mut Health, reply: &glib::Variant) {
    let attrs = glib::VariantDict::new(Some(reply));
    let byte = |key| lookup::<u8>(&attrs, key);
    let count = |key| lookup::<u64>(&attrs, key);
    // "Percentage used" may pass 100 on a worn drive.
    health.percent = byte("percent_used").map(|used| 100 - used.min(100));
    health.available_spare = byte("avail_spare").map(|s| s.min(100));
    health.spare_threshold = byte("spare_thresh").map(|s| s.min(100));
    health.data_read = count("total_data_read");
    health.data_written = count("total_data_written");
    health.power_cycles = count("power_cycles");
    health.unsafe_shutdowns = count("unsafe_shutdowns");
    health.media_errors = count("media_errors");
    health.error_log_entries = count("num_err_log_entries");
    health.minutes_too_hot = lookup::<u32>(&attrs, "warning_temp_time").map(u64::from);
}

/// One ATA attribute from `a(ysqiiixia{sv})`.
#[derive(Clone, Debug, PartialEq)]
struct AtaAttribute {
    id: u8,
    name: String,
    /// Normalised value, worst and threshold, -1 when unknown.
    value: i32,
    threshold: i32,
    /// Raw value interpreted by UDisks, or `None` when it could not.
    pretty: Option<i64>,
}

/// UDisks' units of `pretty`: 1 is a plain count, 3 a number of sectors.
const UNIT_COUNT: i32 = 1;
const UNIT_SECTORS: i32 = 3;

fn ata_attributes(reply: &glib::Variant) -> Vec<AtaAttribute> {
    reply
        .iter()
        .filter_map(|a| {
            let unit = a.child_value(7).get::<i32>()?;
            Some(AtaAttribute {
                id: a.child_value(0).get::<u8>()?,
                name: a.child_value(1).get::<String>()?,
                value: a.child_value(3).get::<i32>()?,
                threshold: a.child_value(5).get::<i32>()?,
                pretty: a
                    .child_value(6)
                    .get::<i64>()
                    .filter(|_| matches!(unit, UNIT_COUNT | UNIT_SECTORS)),
            })
        })
        .collect()
}

/// Attributes about the disk's reliability, whose normalised value the
/// drive lowers as it wears out. Error rates (1, 7, 195) are left out:
/// some vendors' normalised values wander far below 100 on healthy disks.
const RELIABILITY: [u8; 6] = [
    5,   // reallocated sectors
    10,  // spin-up retries
    184, // end-to-end errors
    196, // reallocation events
    197, // pending sectors
    198, // uncorrectable sectors
];

/// SSD life left as a normalised value (100 when new): SSD life left,
/// media wearout indicator, lifetime remaining, wear levelling count,
/// reserved blocks.
const WEAR: [u8; 5] = [231, 233, 202, 177, 169];

/// ATA attributes: sector counts, power cycles and, for SSDs, wear.
fn apply_ata(health: &mut Health, reply: &glib::Variant) {
    let attributes = ata_attributes(reply);
    let pretty = |id: u8| {
        attributes
            .iter()
            .find(|a| a.id == id)
            .and_then(|a| a.pretty)
            .and_then(|p| u64::try_from(p).ok())
    };
    health.reallocated_sectors = pretty(5);
    health.pending_sectors = pretty(197);
    health.uncorrectable_sectors = pretty(198);
    health.power_cycles = pretty(12);
    // Unexpected power loss on SSDs, power-off retracts on hard disks.
    health.unsafe_shutdowns = pretty(174).or_else(|| pretty(192));

    // The drive's own figure when it is an SSD that has one. Some drives
    // keep the temperature in 231; UDisks names it so.
    let wear = (health.rotational == Some(false))
        .then(|| {
            WEAR.iter().find_map(|id| {
                attributes
                    .iter()
                    .find(|a| a.id == *id && !a.name.contains("temperature"))
                    .filter(|a| (1..=100).contains(&a.value))
                    .map(|a| a.value as u8)
            })
        })
        .flatten();
    if let Some(wear) = wear {
        health.percent = Some(wear);
        health.estimated = false;
        return;
    }
    // Otherwise the lowest margin of the reliability attributes above their
    // threshold: 100% while the drive is content, 0% at the threshold,
    // where it calls itself failing.
    let margin = attributes
        .iter()
        .filter(|a| RELIABILITY.contains(&a.id))
        .filter(|a| (1..=253).contains(&a.value) && (0..100).contains(&a.threshold))
        .map(|a| ((a.value - a.threshold) * 100 / (100 - a.threshold)).clamp(0, 100) as u8)
        .min();
    if let Some(margin) = margin {
        health.percent = Some(health.percent.map_or(margin, |p| p.min(margin)));
        health.estimated = true;
    }
}

/// Fill in what follows from the rest: the estimate for drives that give no
/// figure of their own, lowered by every bad sector.
fn finish(health: &mut Health) {
    if health.protocol != Protocol::Ata || (health.percent.is_some() && !health.estimated) {
        return;
    }
    let bad = health
        .bad_sectors
        .or_else(|| Some(health.reallocated_sectors? + health.pending_sectors.unwrap_or(0)));
    if let Some(bad) = bad {
        let by_sectors = 100u64.saturating_sub(bad) as u8;
        health.percent = Some(health.percent.map_or(by_sectors, |p| p.min(by_sectors)));
        health.estimated = true;
    }
    if health.failing {
        health.percent = Some(0);
        health.estimated = true;
    }
}

/// SMART data of every drive, and which block devices are on which drive.
#[derive(Debug, Default)]
pub struct Drives {
    /// By UDisks drive object path.
    pub by_path: HashMap<String, DriveSmart>,
    /// (kernel device name, drive object path).
    blocks: Vec<(String, String)>,
}

impl Drives {
    /// SMART data by kernel device name, partitions included.
    pub fn by_device(&self) -> HashMap<String, DriveSmart> {
        self.blocks
            .iter()
            .filter_map(|(name, drive)| Some((name.clone(), self.by_path.get(drive)?.clone())))
            .collect()
    }
}

/// SMART data from a `GetManagedObjects` reply (`a{oa{sa{sv}}}`): each
/// Block object names its device and its Drive, and the Drive object's
/// `Drive.Ata` or `NVMe.Controller` interface holds the SMART summary.
pub fn parse_objects(objects: &glib::Variant) -> Drives {
    let mut drives = Drives::default();
    let mut rotation: HashMap<String, i32> = HashMap::new();
    for object in objects.iter() {
        let path = object.child_value(0);
        let Some(path) = path.str() else { continue };
        for interface in object.child_value(1).iter() {
            let name = interface.child_value(0);
            let props = glib::VariantDict::new(Some(&interface.child_value(1)));
            match name.str() {
                Some(ATA) => {
                    drives.by_path.insert(path.to_owned(), parse_ata(&props));
                }
                Some(NVME) => {
                    drives.by_path.insert(path.to_owned(), parse_nvme(&props));
                }
                Some("org.freedesktop.UDisks2.Drive") => {
                    // 0 for SSDs, -1 when unknown, otherwise RPM.
                    if let Some(rpm) = lookup::<i32>(&props, "RotationRate") {
                        rotation.insert(path.to_owned(), rpm);
                    }
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
                            drives.blocks.push((name.to_owned(), drive));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    for (path, smart) in &mut drives.by_path {
        if let (Some(health), Some(rpm)) = (&mut smart.health, rotation.get(path)) {
            if health.protocol == Protocol::Ata && *rpm >= 0 {
                health.rotational = Some(*rpm > 0);
            }
        }
    }
    drives
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    type Props = HashMap<String, glib::Variant>;

    /// A `GetManagedObjects` reply with one drive (`D1`) holding `iface`
    /// (and `Drive` with `rpm`), and the block devices `sda` and `sda1`.
    fn objects(iface: &str, drive_props: Props, rpm: i32) -> glib::Variant {
        let mut drive = HashMap::new();
        drive.insert(iface.to_owned(), drive_props);
        let mut drive_iface = Props::new();
        drive_iface.insert("RotationRate".into(), rpm.to_variant());
        drive.insert("org.freedesktop.UDisks2.Drive".to_owned(), drive_iface);

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
        ata.insert(
            "SmartNumAttributesFailedInThePast".into(),
            0i32.to_variant(),
        );
        ata.insert("SmartNumBadSectors".into(), 0i64.to_variant());
        ata.insert("SmartSelftestStatus".into(), "success".to_variant());
        ata
    }

    fn sda(props: Props, rpm: i32) -> DriveSmart {
        parse_objects(&objects(ATA, props, rpm)).by_device()["sda"].clone()
    }

    /// An ATA attribute as UDisks sends it.
    fn attr(id: u8, name: &str, value: i32, thresh: i32, pretty: i64, unit: i32) -> glib::Variant {
        (
            id,
            name.to_owned(),
            0u16,
            value,
            value,
            thresh,
            pretty,
            unit,
            HashMap::<String, glib::Variant>::new(),
        )
            .to_variant()
    }

    fn ata_reply(attrs: &[glib::Variant]) -> glib::Variant {
        glib::Variant::array_from_iter_with_type(attrs[0].type_(), attrs.iter().cloned())
    }

    #[test]
    fn blocks_take_their_drive_temperature() {
        let drives = parse_objects(&objects(ATA, hard_disk(), 7200)).by_device();
        assert!((drives["sda"].temp_c.unwrap() - 37.0).abs() < 0.01);
        assert!(drives.contains_key("sda1"));
    }

    #[test]
    fn healthy_hard_disk() {
        let mut health = sda(hard_disk(), 7200).health.unwrap();
        assert!(health.is_good());
        assert_eq!(health.rotational, Some(true));
        assert_eq!(health.power_on_hours, Some(9000));
        assert_eq!(health.bad_sectors, Some(0));
        let reply = ata_reply(&[
            attr(1, "raw-read-error-rate", 75, 44, 0, UNIT_COUNT),
            attr(5, "reallocated-sector-count", 100, 10, 0, UNIT_SECTORS),
            attr(12, "power-cycle-count", 100, 20, 512, UNIT_COUNT),
            attr(197, "current-pending-sector", 100, 0, 0, UNIT_SECTORS),
        ]);
        apply_attributes(&mut health, &reply);
        finish(&mut health);
        // The wandering read error rate does not count.
        assert_eq!(health.percent, Some(100));
        assert!(health.estimated);
        assert_eq!(health.power_cycles, Some(512));
        assert_eq!(health.reallocated_sectors, Some(0));
    }

    #[test]
    fn bad_sectors_lower_the_estimate() {
        let mut props = hard_disk();
        props.insert("SmartNumBadSectors".into(), 8i64.to_variant());
        props.insert("SmartSelftestStatus".into(), "error_read".to_variant());
        let mut health = sda(props, 5400).health.unwrap();
        let reply = ata_reply(&[attr(5, "reallocated-sector-count", 98, 10, 8, UNIT_SECTORS)]);
        apply_attributes(&mut health, &reply);
        finish(&mut health);
        assert!(!health.is_good());
        assert!(!health.failing);
        assert!(health.selftest_failed);
        // 100 − 8 bad sectors beats the normalised margin of 97%.
        assert_eq!(health.percent, Some(92));
    }

    #[test]
    fn a_failing_disk_is_at_zero() {
        let mut props = hard_disk();
        props.insert("SmartFailing".into(), true.to_variant());
        let mut health = sda(props, 7200).health.unwrap();
        finish(&mut health);
        assert_eq!(health.percent, Some(0));
    }

    #[test]
    fn without_attributes_the_estimate_comes_from_bad_sectors() {
        let mut health = sda(hard_disk(), 7200).health.unwrap();
        finish(&mut health);
        assert_eq!(health.percent, Some(100));
        assert!(health.estimated);
    }

    #[test]
    fn ssd_wear_is_the_drives_own_figure() {
        let mut health = sda(hard_disk(), 0).health.unwrap();
        assert_eq!(health.rotational, Some(false));
        let reply = ata_reply(&[
            attr(5, "reallocated-sector-count", 100, 10, 0, UNIT_SECTORS),
            attr(177, "wear-leveling-count", 87, 0, 120, UNIT_COUNT),
        ]);
        apply_attributes(&mut health, &reply);
        finish(&mut health);
        assert_eq!(health.percent, Some(87));
        assert!(!health.estimated);
    }

    #[test]
    fn a_temperature_in_231_is_no_wear() {
        let mut health = sda(hard_disk(), 0).health.unwrap();
        let reply = ata_reply(&[attr(231, "temperature-celsius", 60, 0, 0, 4)]);
        apply_attributes(&mut health, &reply);
        finish(&mut health);
        assert!(health.estimated);
        assert_eq!(health.percent, Some(100));
    }

    #[test]
    fn unknown_counts_are_none() {
        let mut props = hard_disk();
        props.insert("SmartNumBadSectors".into(), (-1i64).to_variant());
        props.insert("SmartNumAttributesFailing".into(), (-1i32).to_variant());
        let mut health = sda(props, -1).health.unwrap();
        finish(&mut health);
        assert_eq!(health.bad_sectors, None);
        assert_eq!(health.attributes_failing, None);
        assert_eq!(health.rotational, None);
        assert_eq!(health.percent, None);
        assert!(health.is_good());
    }

    #[test]
    fn no_health_when_smart_is_off_or_unread() {
        let mut off = hard_disk();
        off.insert("SmartEnabled".into(), false.to_variant());
        let smart = sda(off, 7200);
        assert_eq!(smart.health, None);
        // The temperature still counts.
        assert!(smart.temp_c.is_some());

        let mut unread = hard_disk();
        unread.insert("SmartUpdated".into(), 0u64.to_variant());
        assert_eq!(sda(unread, 7200).health, None);
    }

    #[test]
    fn nvme_drive() {
        let mut nvme = Props::new();
        nvme.insert("SmartUpdated".into(), 1_700_000_000u64.to_variant());
        nvme.insert("SmartPowerOnHours".into(), 2248u64.to_variant());
        nvme.insert(
            "SmartCriticalWarning".into(),
            vec!["spare".to_owned()].to_variant(),
        );
        nvme.insert("SmartTemperature".into(), 300u16.to_variant());
        let smart = parse_objects(&objects(NVME, nvme, 0)).by_device()["sda"].clone();
        assert_eq!(smart.temp_c, None);
        let mut health = smart.health.unwrap();
        assert_eq!(health.power_on_hours, Some(2248));
        assert_eq!(health.critical_warnings, ["spare"]);
        assert!(!health.is_good());

        let attrs = glib::VariantDict::new(None);
        attrs.insert("percent_used", 3u8);
        attrs.insert("avail_spare", 100u8);
        attrs.insert("spare_thresh", 10u8);
        attrs.insert("total_data_written", 9_595_902_976_000u64);
        attrs.insert("power_cycles", 315u64);
        attrs.insert("unsafe_shutdowns", 80u64);
        attrs.insert("media_errors", 0u64);
        attrs.insert("warning_temp_time", 0u32);
        apply_attributes(&mut health, &attrs.end());
        finish(&mut health);
        assert_eq!(health.percent, Some(97));
        assert!(!health.estimated);
        assert_eq!(health.available_spare, Some(100));
        assert_eq!(health.data_written, Some(9_595_902_976_000));
        assert_eq!(health.power_cycles, Some(315));
        assert_eq!(health.unsafe_shutdowns, Some(80));
    }

    #[test]
    fn a_worn_out_nvme_drive_is_at_zero() {
        let mut health = Health {
            protocol: Protocol::Nvme,
            ..Health::default()
        };
        let attrs = glib::VariantDict::new(None);
        attrs.insert("percent_used", 140u8);
        apply_attributes(&mut health, &attrs.end());
        assert_eq!(health.percent, Some(0));
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
