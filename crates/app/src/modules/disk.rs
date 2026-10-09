//! Disk page: one block per physical disk (activity graph, read and write
//! speed), then the space used on each local filesystem.

use super::{Module, ResourcePage};
use crate::config::HISTORY_LEN;
use crate::i18n::{self, gettext};
use crate::widgets::Graph;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
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
    /// The layout comes from `disk-page.blp`; disk blocks and filesystem
    /// rows are added at run time.
    fn new() -> Self {
        let view = view::DiskView::new();
        let ui = view.imp();
        Self {
            disks_box: ui.disks_box.get(),
            no_disks: ui.no_disks.get(),
            disks: RefCell::default(),
            fs_group: ui.fs_group.get(),
            fs_rows: RefCell::default(),
            root: view.upcast(),
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
            row.set_subtitle(&i18n::fmt(
                // Translators: e.g. "120 GB of 500 GB used · ext4 · /dev/nvme0n1p2"
                &gettext("{used} of {total} used · {type} · {device}"),
                &[
                    ("used", &glib::format_size(fs.space.used)),
                    ("total", &glib::format_size(fs.space.total)),
                    ("type", &fs.fs_type),
                    ("device", &fs.source),
                ],
            ));
            bar.set_value(f64::from(fs.space.used_fraction()));
        }
    }
}

impl ResourcePage for DiskPage {
    fn id(&self) -> &'static str {
        "disk"
    }

    fn title(&self) -> String {
        gettext("Disks")
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
            .label(i18n::fmt(
                // Translators: disk type, size and kernel name, e.g. "NVMe · 512 GB · nvme0n1"
                &gettext("{kind} · {size} · {name}"),
                &[
                    ("kind", &kind_label(device.kind)),
                    ("size", &glib::format_size(device.size)),
                    ("name", &device.name),
                ],
            ))
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();

        let graph = Graph::new(HISTORY_LEN, 100);
        graph.update_property(&[gtk::accessible::Property::Label(&i18n::fmt(
            &gettext("Activity of {model}"),
            &[("model", &device.model)],
        ))]);

        let row = |title: &str| adw::ActionRow::builder().title(title).subtitle("…").build();
        let (read, write) = (row(&gettext("Read")), row(&gettext("Write")));
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
        Some(bps) => i18n::fmt(
            &gettext("{speed} · {total} since boot"),
            &[("speed", &speed(bps)), ("total", &glib::format_size(total))],
        ),
        None => "…".to_owned(),
    }
}

fn speed(bps: u64) -> String {
    // Translators: a transfer speed, e.g. "12.3 MB/s"
    i18n::fmt(&gettext("{size}/s"), &[("size", &glib::format_size(bps))])
}

fn kind_label(kind: DiskKind) -> String {
    match kind {
        DiskKind::Nvme => gettext("NVMe"),
        DiskKind::Ssd => gettext("SSD"),
        DiskKind::Hdd => gettext("Hard disk"),
        DiskKind::Removable => gettext("Removable"),
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

/// The `CwDiskPage` template from `data/ui/stable/disk-page.blp`.
mod view {
    use crate::widgets::Graph;
    use adw::subclass::prelude::*;
    use gtk::prelude::StaticTypeExt;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/disk-page.ui")]
        pub struct DiskView {
            #[template_child]
            pub disks_box: TemplateChild<gtk::Box>,
            #[template_child]
            pub no_disks: TemplateChild<gtk::Label>,
            #[template_child]
            pub fs_group: TemplateChild<adw::PreferencesGroup>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for DiskView {
            const NAME: &'static str = "CwDiskPage";
            type Type = super::DiskView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                Graph::ensure_type();
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for DiskView {}
        impl WidgetImpl for DiskView {}
        impl BinImpl for DiskView {}
    }

    glib::wrapper! {
        pub struct DiskView(ObjectSubclass<imp::DiskView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl DiskView {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
