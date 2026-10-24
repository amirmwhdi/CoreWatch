//! Battery page: charge level graph, status, remaining time, power consumption, health, and hardware.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use crate::widgets::Graph;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::battery::{BatteryCollector, BatteryState};
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use std::cell::Cell;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(BatteryCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(BatteryPage::new())
}

pub const MODULE: Module = Module { collector, page };

struct BatteryPage {
    root: gtk::Widget,
    graph: Graph,
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
            status: ui.status_row.get(),
            level: ui.level_row.get(),
            level_bar: ui.level_bar.get(),
            time: ui.time_row.get(),
            rate: ui.rate_row.get(),
            health: ui.health_row.get(),
            power_supply: ui.power_supply_row.get(),
            hardware: ui.hardware_row.get(),
            static_info_set: Cell::new(false),
            root: view.upcast(),
        }
    }
}

impl ResourcePage for BatteryPage {
    fn id(&self) -> &'static str {
        "battery"
    }

    fn title(&self) -> String {
        gettext("Battery")
    }

    fn icon_name(&self) -> &'static str {
        "battery-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        let Some(battery_sample) = &snapshot.battery else {
            return;
        };
        let Some(battery) = battery_sample.batteries.first() else {
            return;
        };

        self.graph.push(battery.capacity);
        self.level_bar.set_value(f64::from(battery.capacity));

        let status_text = match battery.state {
            BatteryState::Charging => gettext("Charging"),
            BatteryState::Discharging => gettext("Discharging"),
            BatteryState::NotCharging => gettext("Not charging"),
            BatteryState::Full => gettext("Fully charged"),
            BatteryState::Unknown => gettext("Unknown"),
        };
        self.status.set_subtitle(&status_text);

        self.level.set_subtitle(&i18n::percent(battery.capacity));

        // Time remaining
        match battery.time_remaining_secs {
            Some(secs) => {
                let hours = secs / 3600;
                let minutes = (secs % 3600) / 60;
                let time_desc = if hours > 0 {
                    i18n::fmt(&gettext("{h}h {m}m"), &[("h", &hours), ("m", &minutes)])
                } else {
                    i18n::fmt(&gettext("{m} minutes"), &[("m", &minutes)])
                };
                let full_desc = match battery.state {
                    BatteryState::Charging => {
                        i18n::fmt(&gettext("{time} until full"), &[("time", &time_desc)])
                    }
                    _ => i18n::fmt(&gettext("{time} remaining"), &[("time", &time_desc)]),
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
                // Translators: a power draw, e.g. "7.4 W"
                let text = i18n::fmt(&gettext("{watts} W"), &[("watts", &format!("{w:.1}"))]);
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
                let text = i18n::fmt(
                    // Translators: e.g. "92% · 46.1 Wh of 50.0 Wh design"
                    &gettext("{health} · {full} Wh of {design} Wh design"),
                    &[
                        ("health", &i18n::percent(health_pct)),
                        ("full", &format!("{full:.1}")),
                        ("design", &format!("{design:.1}")),
                    ],
                );
                self.health.set_subtitle(&text);
            }
            (Some(health_pct), _, _) => {
                self.health.set_subtitle(&i18n::percent(health_pct));
            }
            // The driver does not report the design capacity: say nothing
            // instead of guessing.
            _ => self.health.set_visible(false),
        }

        // AC adapter
        match battery_sample.ac_online {
            Some(online) => {
                self.power_supply.set_subtitle(&if online {
                    gettext("Connected")
                } else {
                    gettext("Disconnected (Running on battery)")
                });
                self.power_supply.set_visible(true);
            }
            None => self.power_supply.set_visible(false),
        }

        // Static hardware facts
        if !self.static_info_set.get() {
            let cycles_str = battery
                .cycle_count
                .map(|c| i18n::fmt(&gettext("{c} cycles"), &[("c", &c)]))
                .unwrap_or_default();

            let hw_text = if !cycles_str.is_empty() {
                i18n::fmt(
                    &gettext("{model} · {tech} · {cycles}"),
                    &[
                        ("model", &battery.model),
                        ("tech", &battery.technology),
                        ("cycles", &cycles_str),
                    ],
                )
            } else {
                i18n::fmt(
                    &gettext("{model} · {tech}"),
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
        let pct = i18n::percent(dev.capacity);
        match dev.state {
            BatteryState::Charging => {
                Some(i18n::fmt(&gettext("{pct} (Charging)"), &[("pct", &pct)]))
            }
            _ => Some(pct),
        }
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
            pub graph: TemplateChild<Graph>,
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
