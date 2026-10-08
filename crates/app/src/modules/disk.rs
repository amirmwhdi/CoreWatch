//! Disk page: one block per physical disk (activity graph, read and write
//! speed), then the space used on each local filesystem.

use super::{Module, ResourcePage};
use crate::config::HISTORY_LEN;
use crate::widgets::{layout, Graph};
use adw::prelude::*;
use corewatch_core::disk::{DiskCollector, DiskDevice, DiskKind, Filesystem};
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::glib;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::PathBuf;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(DiskCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(DiskPage::new())
}

pub const MODULE: Module = Module { collector, page };

/// Filesystems above this share of used space get the warning colour.
const SPACE_WARNING: f64 = 0.9;

struct DiskPage {
    root: gtk::Widget,
    disks_box: gtk::Box,
    no_disks: gtk::Label,
    /// Widgets per disk, keyed by kernel name; kept while the disk stays
    /// plugged in so its graph keeps its history.
    disks: RefCell<BTreeMap<String, DiskBlock>>,
    fs_group: adw::PreferencesGroup,
    fs_rows: RefCell<Vec<(PathBuf, adw::ActionRow, gtk::LevelBar)>>,
}

impl DiskPage {
    fn new() -> Self {
        let disks_box = gtk::Box::new(gtk::Orientation::Vertical, 24);
        let no_disks = gtk::Label::builder()
            .label("No disks found")
            .css_classes(["dim-label"])
            .visible(false)
            .build();

        let fs_group = adw::PreferencesGroup::builder()
            .title("Filesystems")
            .build();

        let content = layout::page_box();
        content.append(&disks_box);
        content.append(&no_disks);
        content.append(&fs_group);

        Self {
            root: layout::scrolled(&content),
            disks_box,
            no_disks,
            disks: RefCell::default(),
            fs_group,
            fs_rows: RefCell::default(),
        }
    }

    /// Add blocks for new disks and drop blocks for unplugged ones; blocks of
    /// disks that stay keep their widgets.
    fn sync_disks(&self, devices: &[DiskDevice]) {
        let mut blocks = self.disks.borrow_mut();
        let same =
            blocks.len() == devices.len() && devices.iter().all(|d| blocks.contains_key(&d.name));
        if !same {
            blocks.retain(|name, block| {
                let keep = devices.iter().any(|d| &d.name == name);
                if !keep {
                    self.disks_box.remove(&block.container);
                }
                keep
            });
            for device in devices {
                blocks
                    .entry(device.name.clone())
                    .or_insert_with(|| DiskBlock::new(device));
            }
            // Re-append in name order so the layout matches the sorted list.
            for block in blocks.values() {
                if block.container.parent().is_some() {
                    self.disks_box.remove(&block.container);
                }
                self.disks_box.append(&block.container);
            }
            self.no_disks.set_visible(devices.is_empty());
        }
        for device in devices {
            if let Some(block) = blocks.get(&device.name) {
                block.update(device);
            }
        }
    }

    /// Rebuild the rows only when the set of mount points changes; otherwise
    /// just refresh their text.
    fn sync_filesystems(&self, filesystems: &[Filesystem]) {
        let mut rows = self.fs_rows.borrow_mut();
        let same = rows.len() == filesystems.len()
            && rows
                .iter()
                .zip(filesystems)
                .all(|((path, _, _), fs)| path == &fs.mount_point);
        if !same {
            for (_, row, _) in rows.drain(..) {
                self.fs_group.remove(&row);
            }
            for fs in filesystems {
                let row = adw::ActionRow::builder()
                    .title(fs.mount_point.to_string_lossy().as_ref())
                    .subtitle_selectable(true)
                    .build();
                let bar = space_bar();
                row.add_suffix(&bar);
                self.fs_group.add(&row);
                rows.push((fs.mount_point.clone(), row, bar));
            }
            self.fs_group.set_visible(!filesystems.is_empty());
        }
        for ((_, row, bar), fs) in rows.iter().zip(filesystems) {
            row.set_subtitle(&format!(
                "{} of {} used · {} · {}",
                glib::format_size(fs.space.used),
                glib::format_size(fs.space.total),
                fs.fs_type,
                fs.source
            ));
            bar.set_value(f64::from(fs.space.used_fraction()));
        }
    }
}

impl ResourcePage for DiskPage {
    fn id(&self) -> &'static str {
        "disk"
    }

    fn title(&self) -> &'static str {
        "Disks"
    }

    fn icon_name(&self) -> &'static str {
        "drive-harddisk-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        let Some(disk) = &snapshot.disk else { return };
        self.sync_disks(&disk.disks);
        self.sync_filesystems(&disk.filesystems);
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        snapshot.disk.as_ref().map(|d| speed(d.total_bps()))
    }
}

/// Heading, activity graph and the read and write rows of one disk.
struct DiskBlock {
    container: gtk::Box,
    graph: Graph,
    read: adw::ActionRow,
    write: adw::ActionRow,
}

impl DiskBlock {
    fn new(device: &DiskDevice) -> Self {
        let title = gtk::Label::builder()
            .label(device.model.as_str())
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .css_classes(["heading"])
            .build();
        let description = gtk::Label::builder()
            .label(format!(
                "{} · {} · {}",
                kind_label(device.kind),
                glib::format_size(device.size),
                device.name
            ))
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();

        let graph = Graph::new(HISTORY_LEN, 100);
        graph.update_property(&[gtk::accessible::Property::Label(&format!(
            "Activity of {}",
            device.model
        ))]);

        let row = |title: &str| adw::ActionRow::builder().title(title).subtitle("…").build();
        let (read, write) = (row("Read"), row("Write"));
        let rows = adw::PreferencesGroup::new();
        rows.add(&read);
        rows.add(&write);

        let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
        header.append(&title);
        header.append(&description);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&header);
        container.append(&graph);
        container.append(&rows);

        Self {
            container,
            graph,
            read,
            write,
        }
    }

    fn update(&self, device: &DiskDevice) {
        if let Some(busy) = device.busy {
            self.graph.push(busy);
        }
        self.read
            .set_subtitle(&transfer(device.read_bps, device.read_total));
        self.write
            .set_subtitle(&transfer(device.write_bps, device.written_total));
    }
}

/// "12.3 MB/s · 4.1 GB since boot"; "…" while the speed is not known yet.
fn transfer(bps: Option<u64>, total: u64) -> String {
    match bps {
        Some(bps) => format!("{} · {} since boot", speed(bps), glib::format_size(total)),
        None => "…".to_owned(),
    }
}

fn speed(bps: u64) -> String {
    format!("{}/s", glib::format_size(bps))
}

fn kind_label(kind: DiskKind) -> &'static str {
    match kind {
        DiskKind::Nvme => "NVMe",
        DiskKind::Ssd => "SSD",
        DiskKind::Hdd => "Hard disk",
        DiskKind::Removable => "Removable",
    }
}

/// A level bar that keeps the normal colour up to [`SPACE_WARNING`] and turns
/// to the warning colour above it. GTK styles the range below the first offset
/// with that offset's name, so "high" (normal) covers 0–90% and "low"
/// (warning) covers the rest.
fn space_bar() -> gtk::LevelBar {
    let bar = gtk::LevelBar::builder()
        .width_request(120)
        .valign(gtk::Align::Center)
        .build();
    for name in [
        gtk::LEVEL_BAR_OFFSET_LOW,
        gtk::LEVEL_BAR_OFFSET_HIGH,
        gtk::LEVEL_BAR_OFFSET_FULL,
    ] {
        bar.remove_offset_value(Some(name));
    }
    bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_HIGH, SPACE_WARNING);
    bar.add_offset_value(gtk::LEVEL_BAR_OFFSET_LOW, 1.0);
    bar
}
