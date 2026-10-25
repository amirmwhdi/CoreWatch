//! Sensors page: the sensors that belong to no other page (motherboard,
//! case fans, ACPI zones), one group per chip. The others are on their
//! pages through `widgets::SensorGroup`.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use crate::widgets::sensor_group::{readings, value_label, Reading};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::sensors::{Chip, ChipKind, Home, SensorCollector};
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

/// What makes two samples need the same widgets: each chip with the titles
/// and subtitles of its rows, and the units.
type Shape = (Vec<(String, Vec<(String, String)>)>, u32);

struct SensorsPage {
    root: gtk::Widget,
    chips_box: gtk::Box,
    shape: RefCell<Shape>,
    widgets: RefCell<Vec<gtk::Widget>>,
    /// One value label per sensor, in reading order.
    values: RefCell<Vec<gtk::Label>>,
}

impl SensorsPage {
    fn new() -> Self {
        let view = view::SensorsView::new();
        let ui = view.imp();
        Self {
            chips_box: ui.chips_box.get(),
            shape: RefCell::default(),
            widgets: RefCell::default(),
            values: RefCell::default(),
            root: view.upcast(),
        }
    }

    fn rebuild(&self, readings: &[Reading]) {
        for widget in self.widgets.take() {
            self.chips_box.remove(&widget);
        }
        let mut values = Vec::new();
        let mut widgets: Vec<gtk::Widget> = Vec::new();
        if readings.is_empty() {
            let status = adw::StatusPage::builder()
                .icon_name("cw-temperature-symbolic")
                .title(gettext("No Other Sensors"))
                .description(gettext(
                    "The sensors of the processor, graphics card, drives, network and battery are shown on their own pages.",
                ))
                .vexpand(true)
                .build();
            self.chips_box.append(&status);
            widgets.push(status.upcast());
        }
        let mut group: Option<(&Chip, adw::PreferencesGroup)> = None;
        for r in readings {
            if !group
                .as_ref()
                .is_some_and(|(chip, _)| std::ptr::eq(*chip, r.chip))
            {
                let g = adw::PreferencesGroup::builder()
                    .title(chip_title(r.chip))
                    .description(r.chip.model.as_deref().unwrap_or(&r.chip.driver))
                    .build();
                self.chips_box.append(&g);
                widgets.push(g.clone().upcast());
                group = Some((r.chip, g));
            }
            let Some((_, g)) = &group else { continue };
            let row = adw::ActionRow::builder().title(&r.title).build();
            // The group already names the chip.
            if let Some(critical) = &r.critical {
                row.set_subtitle(critical);
            }
            values.push(value_label(&row));
            g.add(&row);
        }
        *self.widgets.borrow_mut() = widgets;
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
        let readings = readings(sensors, Home::Sensors);
        let mut chips: Vec<(String, Vec<(String, String)>)> = Vec::new();
        let mut last: Option<&Chip> = None;
        for r in &readings {
            if !last.is_some_and(|c| std::ptr::eq(c, r.chip)) {
                chips.push((r.chip.driver.clone(), Vec::new()));
                last = Some(r.chip);
            }
            if let Some((_, rows)) = chips.last_mut() {
                rows.push((r.title.clone(), r.critical.clone().unwrap_or_default()));
            }
        }
        let shape: Shape = (chips, i18n::units_generation());
        if *self.shape.borrow() != shape {
            self.rebuild(&readings);
            *self.shape.borrow_mut() = shape;
        }
        for (label, r) in self.values.borrow().iter().zip(&readings) {
            if label.label() != r.value {
                label.set_label(&r.value);
            }
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.sensors.is_some()
    }

    /// The hottest of the sensors shown here; the others have their pages.
    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        let sensors = snapshot.sensors.as_ref()?;
        sensors
            .chips
            .iter()
            .flat_map(|c| {
                c.temperatures
                    .iter()
                    .filter(|t| c.home(t.label.as_deref()) == Home::Sensors)
            })
            .map(|t| t.celsius)
            .max_by(f32::total_cmp)
            .map(i18n::celsius)
    }
}

fn chip_title(chip: &Chip) -> String {
    match chip.kind {
        ChipKind::Cpu => gettext("Processor"),
        ChipKind::Gpu => gettext("Graphics"),
        ChipKind::Storage => gettext("Drive"),
        ChipKind::Board => gettext("Motherboard"),
        ChipKind::Network => gettext("Wireless card"),
        ChipKind::Battery => gettext("Battery"),
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
