//! GPU page: one block per graphics card with its load graph, clocks,
//! video memory, temperature, power and fan, as far as its driver reports
//! them.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use crate::widgets::{Graph, SensorGroup};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::gpu::{GpuCollector, GpuDevice};
use corewatch_core::sensors::Home;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use std::cell::RefCell;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(GpuCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(GpuPage::new())
}

pub const MODULE: Module = Module {
    collector: Some(collector),
    page,
};

struct GpuPage {
    sensors: SensorGroup,
    root: gtk::Widget,
    gpus_box: gtk::Box,
    /// One block per card, in sample order. Cards are found once at start,
    /// so the blocks are built once.
    blocks: RefCell<Vec<(String, GpuBlock)>>,
}

impl GpuPage {
    fn new() -> Self {
        let view = view::GpuView::new();
        let ui = view.imp();
        Self {
            gpus_box: ui.gpus_box.get(),
            blocks: RefCell::default(),
            sensors: SensorGroup::new(ui.sensors_group.get(), Home::Gpu),
            root: view.upcast(),
        }
    }
}

impl ResourcePage for GpuPage {
    fn id(&self) -> &'static str {
        "gpu"
    }

    fn title(&self) -> String {
        gettext("Graphics")
    }

    fn icon_name(&self) -> &'static str {
        "video-display-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        self.sensors.update(snapshot.sensors.as_ref());
        let Some(gpu) = &snapshot.gpu else { return };
        let mut blocks = self.blocks.borrow_mut();
        let same = blocks.len() == gpu.devices.len()
            && blocks
                .iter()
                .zip(&gpu.devices)
                .all(|((id, _), d)| id == &d.id);
        if !same {
            for (_, block) in blocks.drain(..) {
                self.gpus_box.remove(&block.container);
            }
            for device in &gpu.devices {
                let block = GpuBlock::new(device);
                self.gpus_box.append(&block.container);
                blocks.push((device.id.clone(), block));
            }
        }
        for ((_, block), device) in blocks.iter().zip(&gpu.devices) {
            block.update(device);
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.gpu.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        let gpu = snapshot.gpu.as_ref()?;
        if let Some(usage) = gpu.max_usage() {
            return Some(i18n::percent(usage));
        }
        let first = gpu.devices.first()?;
        if first.asleep {
            return Some(gettext("Asleep"));
        }
        first.freq_mhz.map(mhz)
    }
}

/// Heading, graph and value rows of one card. Rows the driver has no value
/// for stay hidden.
struct GpuBlock {
    container: gtk::Box,
    graph: Graph,
    status: adw::ActionRow,
    usage: adw::ActionRow,
    clock: adw::ActionRow,
    mem_clock: adw::ActionRow,
    vram: adw::ActionRow,
    vram_bar: gtk::LevelBar,
    temp: adw::ActionRow,
    power: adw::ActionRow,
    fan: adw::ActionRow,
}

impl GpuBlock {
    fn new(device: &GpuDevice) -> Self {
        let title = gtk::Label::builder()
            .label(device.name.as_str())
            .xalign(0.0)
            .wrap(true)
            .selectable(true)
            .css_classes(["heading"])
            .build();
        let driver = device.driver.clone().unwrap_or_else(|| gettext("Unknown"));
        let description = gtk::Label::builder()
            .label(i18n::fmt(
                // Translators: e.g. "Driver amdgpu · 0000:01:00.0"
                &gettext("Driver {driver} · {id}"),
                &[("driver", &driver), ("id", &device.id)],
            ))
            .xalign(0.0)
            .selectable(true)
            .css_classes(["dim-label", "caption"])
            .build();
        let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
        header.append(&title);
        header.append(&description);

        let graph = Graph::new(140);
        graph.update_property(&[gtk::accessible::Property::Label(&i18n::fmt(
            &gettext("Load of {model}"),
            &[("model", &device.name)],
        ))]);

        let rows = adw::PreferencesGroup::new();
        let row = |title: &str| {
            let row = adw::ActionRow::builder()
                .title(title)
                .subtitle("…")
                .subtitle_selectable(true)
                .visible(false)
                .build();
            rows.add(&row);
            row
        };
        let status = row(&gettext("Status"));
        status.set_subtitle(&gettext("Asleep to save power; values show when it wakes"));
        let usage = row(&gettext("Load"));
        let clock = row(&gettext("Clock Speed"));
        let mem_clock = row(&gettext("Memory Clock"));
        let vram = row(&gettext("Memory (VRAM)"));
        let vram_bar = gtk::LevelBar::builder()
            .width_request(120)
            .valign(gtk::Align::Center)
            .build();
        vram.add_suffix(&vram_bar);
        let temp = row(&gettext("Temperature"));
        let power = row(&gettext("Power"));
        let fan = row(&gettext("Fan"));

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&header);
        container.append(&graph);
        container.append(&rows);
        Self {
            container,
            graph,
            status,
            usage,
            clock,
            mem_clock,
            vram,
            vram_bar,
            temp,
            power,
            fan,
        }
    }

    fn update(&self, d: &GpuDevice) {
        // Load when the driver reports it; nouveau and older Intel chips
        // without RC6 report only the clock, so the graph shows the clock's
        // share of its maximum there.
        let graph_value = if d.asleep {
            Some(0.0)
        } else if d.usage_pending {
            None
        } else {
            d.usage.or_else(|| d.freq_fraction())
        };
        if let Some(value) = graph_value {
            self.graph.push(value);
        }
        self.status.set_visible(d.asleep);

        set(
            &self.usage,
            (!d.asleep).then(|| match d.usage {
                Some(usage) => i18n::percent(usage),
                None if d.usage_pending => "…".to_owned(),
                None => gettext("Not reported by the driver"),
            }),
        );
        set(&self.clock, pair(d.freq_mhz, d.max_freq_mhz, mhz));
        set(
            &self.mem_clock,
            pair(d.mem_freq_mhz, d.mem_max_freq_mhz, mhz),
        );
        let vram = match (d.vram_used, d.vram_total) {
            (Some(used), Some(total)) => Some(i18n::fmt(
                &gettext("{used} of {total}"),
                &[("used", &i18n::size(used)), ("total", &i18n::size(total))],
            )),
            _ => None,
        };
        set(&self.vram, vram);
        self.vram_bar
            .set_value(f64::from(d.vram_fraction().unwrap_or(0.0)));
        set(&self.temp, d.temp_c.map(|t| i18n::celsius(t as f32)));
        set(&self.power, pair(d.power_w, d.power_cap_w, watts));
        let fan = d
            .fan_rpm
            .map(rpm)
            .or_else(|| d.fan_percent.map(i18n::percent));
        set(&self.fan, fan);
    }
}

/// Show `row` with `text`, or hide it when there is none.
fn set(row: &adw::ActionRow, text: Option<String>) {
    row.set_visible(text.is_some());
    if let Some(text) = text {
        if row.subtitle().as_deref() != Some(text.as_str()) {
            row.set_subtitle(&text);
        }
    }
}

/// "1150 MHz / 1366 MHz", or just the current value without a maximum.
fn pair<T: Copy>(now: Option<T>, max: Option<T>, fmt: fn(T) -> String) -> Option<String> {
    let now = now?;
    Some(match max {
        Some(max) => i18n::fmt(
            // Translators: a current value and its maximum, e.g. "1150 MHz / 1366 MHz"
            &gettext("{now} / {max}"),
            &[("now", &fmt(now)), ("max", &fmt(max))],
        ),
        None => fmt(now),
    })
}

fn mhz(value: u32) -> String {
    i18n::fmt(&gettext("{cur} MHz"), &[("cur", &value)])
}

fn watts(value: f32) -> String {
    // Translators: power in watts, e.g. "98 W"
    i18n::fmt(&gettext("{value} W"), &[("value", &format!("{value:.0}"))])
}

fn rpm(value: u32) -> String {
    crate::widgets::sensor_group::rpm(value)
}

/// The `CwGpuPage` template from `data/ui/stable/gpu-page.blp`.
mod view {
    use adw::subclass::prelude::*;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/gpu-page.ui")]
        pub struct GpuView {
            #[template_child]
            pub sensors_group: TemplateChild<adw::PreferencesGroup>,
            #[template_child]
            pub gpus_box: TemplateChild<gtk::Box>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for GpuView {
            const NAME: &'static str = "CwGpuPage";
            type Type = super::GpuView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for GpuView {}
        impl WidgetImpl for GpuView {}
        impl BinImpl for GpuView {}
    }

    glib::wrapper! {
        pub struct GpuView(ObjectSubclass<imp::GpuView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl GpuView {
        pub fn new() -> Self {
            glib::Object::builder().build()
        }
    }
}
