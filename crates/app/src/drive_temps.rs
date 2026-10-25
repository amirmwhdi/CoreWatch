//! Drive temperatures from UDisks, for drives the kernel gives no sensor.
//!
//! The kernel shows a SATA drive's temperature only when the `drivetemp`
//! module is loaded, which most distributions do not do. UDisks reads the
//! drive's SMART data itself (as root, about every ten minutes) and offers
//! the temperature on the system bus to any user, so Corewatch asks it once
//! a minute. The Disks page prefers the kernel's sensor when there is one.

use gtk::{gio, glib};
use std::cell::RefCell;
use std::collections::HashMap;
use std::time::Duration;

/// UDisks refreshes SMART data every ten minutes; once a minute is plenty.
const REFRESH: Duration = Duration::from_secs(60);

thread_local! {
    /// °C by kernel device name (`"sda"`).
    static TEMPS: RefCell<HashMap<String, f32>> = RefCell::default();
}

/// The latest UDisks temperature of `/dev/<name>`.
pub fn celsius(name: &str) -> Option<f32> {
    TEMPS.with(|t| t.borrow().get(name).copied())
}

/// Ask UDisks now and then every [`REFRESH`]. Without UDisks (or without
/// the permission in Flatpak) this logs once and stops.
pub fn start() {
    glib::spawn_future_local(async {
        let bus = match gio::bus_get_future(gio::BusType::System).await {
            Ok(bus) => bus,
            Err(e) => return tracing::debug!("no system bus for drive temperatures: {e}"),
        };
        loop {
            match managed_objects(&bus).await {
                Ok(objects) => {
                    let temps = parse_objects(&objects);
                    TEMPS.with(|t| *t.borrow_mut() = temps);
                }
                Err(e) => return tracing::debug!("UDisks unavailable: {e}"),
            }
            glib::timeout_future(REFRESH).await;
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

/// Temperatures by device name from a `GetManagedObjects` reply
/// (`a{oa{sa{sv}}}`): each Block object names its device and its Drive, and
/// the Drive's `Ata` interface holds `SmartTemperature` in kelvin (0 when
/// unknown).
pub fn parse_objects(objects: &glib::Variant) -> HashMap<String, f32> {
    let mut drive_temps: HashMap<String, f32> = HashMap::new();
    let mut blocks: Vec<(String, String)> = Vec::new();
    for object in objects.iter() {
        let path = object.child_value(0);
        let Some(path) = path.str() else { continue };
        for interface in object.child_value(1).iter() {
            let name = interface.child_value(0);
            let props = glib::VariantDict::new(Some(&interface.child_value(1)));
            match name.str() {
                Some("org.freedesktop.UDisks2.Drive.Ata") => {
                    let kelvin = props
                        .lookup_value("SmartTemperature", None)
                        .and_then(|v| v.get::<f64>())
                        .filter(|k| *k > 0.0);
                    if let Some(kelvin) = kelvin {
                        drive_temps.insert(path.to_owned(), (kelvin - 273.15) as f32);
                    }
                }
                Some("org.freedesktop.UDisks2.Block") => {
                    let device = props
                        .lookup_value("Device", None)
                        .and_then(|v| v.get::<Vec<u8>>())
                        .map(|bytes| {
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
        .filter_map(|(name, drive)| Some((name, *drive_temps.get(&drive)?)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gtk::prelude::*;

    #[test]
    fn blocks_take_their_drive_temperature() {
        type Props = HashMap<String, glib::Variant>;
        let mut ata = Props::new();
        ata.insert("SmartTemperature".into(), 310.15f64.to_variant());
        let mut drive = HashMap::new();
        drive.insert("org.freedesktop.UDisks2.Drive.Ata".to_owned(), ata);

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

        let temps = parse_objects(&objects.to_variant());
        assert!((temps["sda"] - 37.0).abs() < 0.01);
        assert!(temps.contains_key("sda1"));
    }
}
