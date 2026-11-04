//! Battery page: charge level graph, status, remaining time, power consumption, health, and hardware.

use super::{Module, ResourcePage};
use crate::format;
use crate::widgets::{Graph, SensorGroup};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::battery::{BatteryCollector, BatteryState};
use corewatch_core::sensors::Home;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::glib;
use std::cell::Cell;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(BatteryCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(BatteryPage::new())
}

pub const MODULE: Module = Module {
    collector: Some(collector),
    page,
};

struct BatteryPage {
    sensors: SensorGroup,
    root: gtk::Widget,
    graph: Graph,
    day_box: gtk::Box,
    day_graph: Graph,
    /// The battery whose day history is being shown.
    day_for: std::cell::RefCell<Option<String>>,
    status: adw::ActionRow,
    level: adw::ActionRow,
    level_bar: gtk::LevelBar,
    time: adw::ActionRow,
    rate: adw::ActionRow,
    health: adw::ActionRow,
    power_supply: adw::ActionRow,
    hardware: adw::ActionRow,
    static_info_set: Cell<bool>,
}

impl BatteryPage {
    fn new() -> Self {
        let view = view::BatteryView::new();
        let ui = view.imp();
        Self {
            graph: ui.graph.get(),
            day_box: ui.day_box.get(),
            day_graph: ui.day_graph.get(),
            day_for: std::cell::RefCell::default(),
            status: ui.status_row.get(),
            level: ui.level_row.get(),
            level_bar: ui.level_bar.get(),
            time: ui.time_row.get(),
            rate: ui.rate_row.get(),
            health: ui.health_row.get(),
            power_supply: ui.power_supply_row.get(),
            hardware: ui.hardware_row.get(),
            static_info_set: Cell::new(false),
            sensors: SensorGroup::new(ui.sensors_group.get(), Home::Battery),
            root: view.upcast(),
        }
    }
}

impl BatteryPage {
    /// Ask UPower for the last day of `name` now and every few minutes, as
    /// long as the page exists.
    fn follow_day(&self, name: &str) {
        if self.day_for.borrow().as_deref() == Some(name) {
            return;
        }
        *self.day_for.borrow_mut() = Some(name.to_owned());
        let name = name.to_owned();
        let (day_box, graph) = (self.day_box.downgrade(), self.day_graph.downgrade());
        glib::spawn_future_local(async move {
            loop {
                let values = crate::battery_history::charge(&name).await;
                let (Some(day_box), Some(graph)) = (day_box.upgrade(), graph.upgrade()) else {
                    break;
                };
                day_box.set_visible(values.len() >= 2);
                if values.len() >= 2 {
                    graph.set_values(&values);
                }
                glib::timeout_future(crate::battery_history::REFRESH).await;
            }
        });
    }
}

impl ResourcePage for BatteryPage {
    fn id(&self) -> &'static str {
        "battery"
    }

    fn title(&self) -> String {
        String::from("Battery")
    }

    fn icon_name(&self) -> &'static str {
        "battery-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        self.sensors.update(snapshot.sensors.as_ref());
        let Some(battery_sample) = &snapshot.battery else {
            return;
        };
        let Some(battery) = battery_sample.batteries.first() else {
            return;
        };

        self.graph.push(battery.capacity);
        self.follow_day(&battery.name);
        self.level_bar.set_value(f64::from(battery.capacity));

        self.status.set_subtitle(&state_label(battery.state));

        self.level.set_subtitle(&format::percent(battery.capacity));

        // Time remaining
        match battery.time_remaining_secs {
            Some(secs) => {
                let hours = secs / 3600;
                let minutes = (secs % 3600) / 60;
                let time_desc = if hours > 0 {
                    format::fmt("{h}h {m}m", &[("h", &hours), ("m", &minutes)])
                } else {
                    format::fmt("{m} minutes", &[("m", &minutes)])
                };
                let full_desc = match battery.state {
                    BatteryState::Charging => {
                        format::fmt("{time} until full", &[("time", &time_desc)])
                    }
                    _ => format::fmt("{time} remaining", &[("time", &time_desc)]),
                };
                self.time.set_subtitle(&full_desc);
                self.time.set_visible(true);
            }
            None => {
                self.time.set_visible(false);
            }
        }

        // Power rate
        match battery.power_w {
            Some(w) => {
                // a power draw, e.g. "7.4 W"
                let text = format::fmt("{watts} W", &[("watts", &format!("{w:.1}"))]);
                self.rate.set_subtitle(&text);
                self.rate.set_visible(true);
            }
            None => {
                self.rate.set_visible(false);
            }
        }

        // Health & Capacity
        match (
            battery.health,
            battery.energy_full_wh,
            battery.energy_design_wh,
        ) {
            (Some(health_pct), Some(full), Some(design)) => {
                let text = format::fmt(
                    // e.g. "92% · 46.1 Wh of 50.0 Wh design"
                    "{health} · {full} Wh of {design} Wh design",
                    &[
                        ("health", &format::percent(health_pct)),
                        ("full", &format!("{full:.1}")),
                        ("design", &format!("{design:.1}")),
                    ],
                );
                self.health.set_subtitle(&text);
            }
            (Some(health_pct), _, _) => {
                self.health.set_subtitle(&format::percent(health_pct));
            }
            // The driver does not report the design capacity: say nothing
            // instead of guessing.
            _ => self.health.set_visible(false),
        }

        // AC adapter
        match battery_sample.ac_online {
            Some(online) => {
                self.power_supply.set_subtitle(&if online {
                    String::from("Connected")
                } else {
                    String::from("Disconnected (Running on battery)")
                });
                self.power_supply.set_visible(true);
            }
            None => self.power_supply.set_visible(false),
        }

        // Static hardware facts
        if !self.static_info_set.get() {
            let cycles_str = battery
                .cycle_count
                .map(|c| format::fmt("{c} cycles", &[("c", &c)]))
                .unwrap_or_default();

            let hw_text = if !cycles_str.is_empty() {
                format::fmt(
                    "{model} · {tech} · {cycles}",
                    &[
                        ("model", &battery.model),
                        ("tech", &battery.technology),
                        ("cycles", &cycles_str),
                    ],
                )
            } else {
                format::fmt(
                    "{model} · {tech}",
                    &[("model", &battery.model), ("tech", &battery.technology)],
                )
            };
            self.hardware.set_subtitle(&hw_text);
            self.static_info_set.set(true);
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.battery.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        let b = snapshot.battery.as_ref()?;
        let dev = b.batteries.first()?;
        let pct = format::percent(dev.capacity);
        match dev.state {
            BatteryState::Charging => Some(format::fmt("{pct} (Charging)", &[("pct", &pct)])),
            _ => Some(pct),
        }
    }
}

pub fn state_label(state: BatteryState) -> String {
    match state {
        BatteryState::Charging => String::from("Charging"),
        BatteryState::Discharging => String::from("Discharging"),
        BatteryState::NotCharging => String::from("Not charging"),
        BatteryState::Full => String::from("Fully charged"),
        BatteryState::Unknown => String::from("Unknown"),
    }
}

/// The `CwBatteryPage` template from `data/ui/stable/battery-page.blp`.
mod view {
    use crate::widgets::Graph;
    use adw::subclass::prelude::*;
    use gtk::prelude::StaticTypeExt;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/battery-page.ui")]
        pub struct BatteryView {
            #[template_child]
            pub sensors_group: TemplateChild<adw::PreferencesGroup>,
            #[template_child]
            pub graph: TemplateChild<Graph>,
            #[template_child]
            pub day_box: TemplateChild<gtk::Box>,
            #[template_child]
            pub day_graph: TemplateChild<Graph>,
            #[template_child]
            pub status_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub level_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub level_bar: TemplateChild<gtk::LevelBar>,
            #[template_child]
            pub time_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub rate_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub health_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub power_supply_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub hardware_row: TemplateChild<adw::ActionRow>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for BatteryView {
            const NAME: &'static str = "CwBatteryPage";
            type Type = super::BatteryView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                Graph::ensure_type();
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for BatteryView {}
        impl WidgetImpl for BatteryView {}
        impl BinImpl for BatteryView {}
    }

    glib::wrapper! {
        pub struct BatteryView(ObjectSubclass<imp::BatteryView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl BatteryView {
        pub fn new() -> Self {
            glib::Object::builder().build()
        }
    }
}
