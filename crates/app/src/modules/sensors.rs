//! Sensors page: one group per chip with its temperatures and fan speeds.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::sensors::{Chip, ChipKind, SensorCollector};
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use std::cell::RefCell;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(SensorCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(SensorsPage::new())
}

pub const MODULE: Module = Module {
    collector: Some(collector),
    page,
};

/// What makes two samples need the same widgets: chips, and how many
/// sensors each has.
type Shape = Vec<(String, usize, usize)>;

struct SensorsPage {
    root: gtk::Widget,
    chips_box: gtk::Box,
    shape: RefCell<Shape>,
    groups: RefCell<Vec<adw::PreferencesGroup>>,
    /// One value label per sensor, in sample order: temperatures, then fans.
    values: RefCell<Vec<Vec<gtk::Label>>>,
}

impl SensorsPage {
    fn new() -> Self {
        let view = view::SensorsView::new();
        let ui = view.imp();
        Self {
            chips_box: ui.chips_box.get(),
            shape: RefCell::default(),
            groups: RefCell::default(),
            values: RefCell::default(),
            root: view.upcast(),
        }
    }

    fn rebuild(&self, chips: &[Chip]) {
        for group in self.groups.take() {
            self.chips_box.remove(&group);
        }
        let mut values = Vec::new();
        let mut groups = Vec::new();
        for chip in chips {
            let group = adw::PreferencesGroup::builder()
                .title(chip_title(chip))
                .description(chip.model.as_deref().unwrap_or(&chip.driver))
                .build();
            let mut labels = Vec::new();
            for (i, t) in chip.temperatures.iter().enumerate() {
                let title = t
                    .label
                    .clone()
                    .unwrap_or_else(|| numbered(&gettext("Temperature"), i));
                let row = adw::ActionRow::builder().title(title).build();
                if let Some(critical) = t.critical {
                    row.set_subtitle(&i18n::fmt(
                        &gettext("Critical at {value}"),
                        &[("value", &i18n::celsius(critical))],
                    ));
                }
                labels.push(value_label(&row));
                group.add(&row);
            }
            for (i, f) in chip.fans.iter().enumerate() {
                let title = f
                    .label
                    .clone()
                    .unwrap_or_else(|| numbered(&gettext("Fan"), i));
                let row = adw::ActionRow::builder().title(title).build();
                labels.push(value_label(&row));
                group.add(&row);
            }
            self.chips_box.append(&group);
            groups.push(group);
            values.push(labels);
        }
        *self.groups.borrow_mut() = groups;
        *self.values.borrow_mut() = values;
    }
}

impl ResourcePage for SensorsPage {
    fn id(&self) -> &'static str {
        "sensors"
    }

    fn title(&self) -> String {
        gettext("Sensors")
    }

    fn icon_name(&self) -> &'static str {
        "cw-temperature-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        let Some(sensors) = &snapshot.sensors else {
            return;
        };
        let shape: Shape = sensors
            .chips
            .iter()
            .map(|c| (c.driver.clone(), c.temperatures.len(), c.fans.len()))
            .collect();
        if *self.shape.borrow() != shape {
            self.rebuild(&sensors.chips);
            *self.shape.borrow_mut() = shape;
        }
        for (chip, labels) in sensors.chips.iter().zip(self.values.borrow().iter()) {
            let texts = chip
                .temperatures
                .iter()
                .map(|t| i18n::celsius(t.celsius))
                .chain(chip.fans.iter().map(|f| rpm(f.rpm)));
            for (label, text) in labels.iter().zip(texts) {
                if label.label() != text {
                    label.set_label(&text);
                }
            }
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.sensors.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        let sensors = snapshot.sensors.as_ref()?;
        sensors
            .cpu_celsius()
            .or_else(|| sensors.hottest_celsius())
            .map(i18n::celsius)
    }
}

fn value_label(row: &adw::ActionRow) -> gtk::Label {
    let label = gtk::Label::builder()
        .label("…")
        .valign(gtk::Align::Center)
        .css_classes(["numeric"])
        .build();
    row.add_suffix(&label);
    label
}

/// "Fan 2" for the second unlabeled fan.
fn numbered(what: &str, index: usize) -> String {
    i18n::fmt(
        // Translators: an unnamed sensor and its number, e.g. "Fan 2"
        &gettext("{what} {number}"),
        &[("what", &what), ("number", &(index + 1))],
    )
}

fn rpm(value: u32) -> String {
    // Translators: fan speed in revolutions per minute, e.g. "1200 RPM"
    i18n::fmt(&gettext("{value} RPM"), &[("value", &value)])
}

fn chip_title(chip: &Chip) -> String {
    match chip.kind {
        ChipKind::Cpu => gettext("Processor"),
        ChipKind::Gpu => gettext("Graphics"),
        ChipKind::Storage => gettext("Drive"),
        ChipKind::Board => gettext("Motherboard"),
        ChipKind::Network => gettext("Wireless card"),
        ChipKind::Other => chip.driver.clone(),
    }
}

/// The `CwSensorsPage` template from `data/ui/stable/sensors-page.blp`.
mod view {
    use adw::subclass::prelude::*;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/sensors-page.ui")]
        pub struct SensorsView {
            #[template_child]
            pub chips_box: TemplateChild<gtk::Box>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for SensorsView {
            const NAME: &'static str = "CwSensorsPage";
            type Type = super::SensorsView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for SensorsView {}
        impl WidgetImpl for SensorsView {}
        impl BinImpl for SensorsView {}
    }

    glib::wrapper! {
        pub struct SensorsView(ObjectSubclass<imp::SensorsView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl SensorsView {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
