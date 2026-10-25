//! Overview page: one card per resource with its main number and a small
//! graph. A card opens its resource's page.
//!
//! This page reads every field of the snapshot and has no collector: it only
//! repeats what the other modules collected.

use super::{battery, network, Module, ResourcePage};
use crate::config::HISTORY_LEN;
use crate::i18n::{self, gettext};
use crate::widgets::Graph;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::Snapshot;

fn page() -> Box<dyn ResourcePage> {
    Box::new(OverviewPage::new())
}

pub const MODULE: Module = Module {
    collector: None,
    page,
};

/// Temperatures are drawn as a share of this many degrees.
const TEMPERATURE_TOP: f32 = 100.0;

struct OverviewPage {
    root: gtk::Widget,
    cpu: Card,
    memory: Card,
    gpu: Card,
    disk: Card,
    network: Card,
    sensors: Card,
    battery: Card,
}

impl OverviewPage {
    fn new() -> Self {
        let view = view::OverviewView::new();
        let cards = view.imp().cards.get();
        let card = |page: &str, title: String, icon: &str| Card::new(&cards, page, &title, icon);
        Self {
            cpu: card("cpu", gettext("Processor"), "computer-symbolic"),
            memory: card("memory", gettext("Memory"), "drive-multidisk-symbolic"),
            gpu: card("gpu", gettext("Graphics"), "video-display-symbolic"),
            disk: card("disk", gettext("Disks"), "drive-harddisk-symbolic"),
            network: Card::new_autoscale(
                &cards,
                "network",
                &gettext("Network"),
                "network-wireless-symbolic",
            ),
            sensors: card("sensors", gettext("Temperature"), "cw-temperature-symbolic"),
            battery: card("battery", gettext("Battery"), "battery-symbolic"),
            root: view.upcast(),
        }
    }
}

impl ResourcePage for OverviewPage {
    fn id(&self) -> &'static str {
        "overview"
    }

    fn title(&self) -> String {
        gettext("Overview")
    }

    fn icon_name(&self) -> &'static str {
        "view-grid-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, s: &Snapshot) {
        if let Some(cpu) = &s.cpu {
            let mut details = Vec::new();
            if let Some(mhz) = cpu.max_freq_mhz() {
                details.push(ghz(mhz));
            }
            if let Some(t) = s.sensors.as_ref().and_then(|s| s.cpu_celsius()) {
                details.push(i18n::celsius(t));
            }
            self.cpu.set(
                Some(cpu.usage),
                &i18n::percent(cpu.usage),
                &details.join(" · "),
            );
        }

        if let Some(m) = &s.memory {
            self.memory.set(
                Some(m.used_fraction()),
                &i18n::percent(m.used_fraction()),
                &i18n::fmt(
                    &gettext("{used} of {total}"),
                    &[
                        ("used", &i18n::size(m.used)),
                        ("total", &i18n::size(m.total)),
                    ],
                ),
            );
        }

        if let Some(g) = s.gpu.as_ref().and_then(|g| g.devices.first()) {
            // Like the GPU page: usage when the driver reports it, else the clock.
            let value = match (g.usage, g.freq_mhz) {
                (Some(usage), _) => i18n::percent(usage),
                (None, Some(mhz)) => i18n::fmt(&gettext("{cur} MHz"), &[("cur", &mhz)]),
                (None, None) => "—".to_owned(),
            };
            self.gpu
                .set(g.usage.or_else(|| g.freq_fraction()), &value, &g.name);
        }

        if let Some(d) = &s.disk {
            let busiest = d.disks.iter().filter_map(|d| d.busy).fold(0.0, f32::max);
            let sum = |f: fn(&corewatch_core::disk::DiskDevice) -> Option<u64>| {
                d.disks.iter().filter_map(f).fold(0, u64::saturating_add)
            };
            self.disk.set(
                Some(busiest),
                &i18n::speed(d.total_bps()),
                &i18n::fmt(
                    &gettext("Read {read} · Write {write}"),
                    &[
                        ("read", &i18n::speed(sum(|d| d.read_bps))),
                        ("write", &i18n::speed(sum(|d| d.write_bps))),
                    ],
                ),
            );
        }

        if let Some(n) = &s.network {
            self.network
                .graph
                .push_pair(n.rx_bps() as f32, n.tx_bps() as f32);
            self.network.set(
                None,
                &i18n::fmt(
                    // Translators: download speed, e.g. "↓ 1.2 MiB/s"
                    &gettext("↓ {speed}"),
                    &[("speed", &i18n::speed(n.rx_bps()))],
                ),
                &i18n::fmt(
                    // Translators: upload speed, e.g. "↑ 40 KiB/s"
                    &gettext("↑ {speed}"),
                    &[("speed", &i18n::speed(n.tx_bps()))],
                ),
            );
        }

        if let Some(sensors) = &s.sensors {
            if let Some(hottest) = sensors.hottest_celsius() {
                self.sensors.set(
                    Some(hottest / TEMPERATURE_TOP),
                    &i18n::celsius(hottest),
                    &gettext("Hottest sensor"),
                );
            }
        }

        if let Some(b) = s.battery.as_ref().and_then(|b| b.batteries.first()) {
            self.battery.set(
                Some(b.capacity),
                &i18n::percent(b.capacity),
                &battery::state_label(b.state),
            );
        }
    }

    fn has_data(&self, s: &Snapshot) -> bool {
        s.cpu.is_some() || s.memory.is_some()
    }

    fn summary(&self, _: &Snapshot) -> Option<String> {
        None
    }
}

/// "3.4 GHz"
fn ghz(mhz: u32) -> String {
    let value = format!("{:.1}", f64::from(mhz) / 1000.0);
    // Translators: a processor clock, e.g. "3.4 GHz"
    i18n::fmt(&gettext("{value} GHz"), &[("value", &value)])
}

/// A button showing one resource. Hidden until its data first arrives.
struct Card {
    button: gtk::Button,
    value: gtk::Label,
    detail: gtk::Label,
    graph: Graph,
}

impl Card {
    fn new(cards: &gtk::FlowBox, page: &str, title: &str, icon: &str) -> Self {
        Self::with_graph(cards, page, title, icon, Graph::new(HISTORY_LEN, 56))
    }

    fn new_autoscale(cards: &gtk::FlowBox, page: &str, title: &str, icon: &str) -> Self {
        let graph = Graph::new_autoscale(HISTORY_LEN, 56, network::GRAPH_FLOOR);
        Self::with_graph(cards, page, title, icon, graph)
    }

    fn with_graph(cards: &gtk::FlowBox, page: &str, title: &str, icon: &str, graph: Graph) -> Self {
        let heading = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        heading.append(&gtk::Image::from_icon_name(icon));
        heading.append(
            &gtk::Label::builder()
                .label(title)
                .xalign(0.0)
                .css_classes(["heading"])
                .build(),
        );
        let value = gtk::Label::builder()
            .label("…")
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["cw-value", "numeric"])
            .build();
        let detail = gtk::Label::builder()
            .xalign(0.0)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .css_classes(["dim-label", "caption"])
            .build();
        // The card is the button; the small graph inside is decoration.
        graph.remove_css_class("card");
        graph.set_can_target(false);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 6);
        content.append(&heading);
        content.append(&value);
        content.append(&detail);
        content.append(&graph);

        let button = gtk::Button::builder()
            .child(&content)
            .width_request(180)
            .css_classes(["card", "cw-card"])
            .action_name("app.show-page")
            .action_target(&page.to_variant())
            .tooltip_text(i18n::fmt(&gettext("Open {page}"), &[("page", &title)]))
            .visible(false)
            .build();
        cards.append(&button);
        // Focus the card itself, not the FlowBox cell around it.
        if let Some(cell) = button.parent() {
            cell.set_focusable(false);
            cell.set_visible(false);
        }
        Self {
            button,
            value,
            detail,
            graph,
        }
    }

    /// Show the card with new numbers. `fraction` feeds the graph; `None`
    /// leaves the graph to the caller (two-line graphs).
    fn set(&self, fraction: Option<f32>, value: &str, detail: &str) {
        if !self.button.is_visible() {
            self.button.set_visible(true);
            if let Some(cell) = self.button.parent() {
                cell.set_visible(true);
            }
        }
        if let Some(fraction) = fraction {
            self.graph.push(fraction);
        }
        if self.value.label() != value {
            self.value.set_label(value);
        }
        if self.detail.label() != detail {
            self.detail.set_label(detail);
        }
    }
}

/// The `CwOverviewPage` template from `data/ui/stable/overview-page.blp`.
mod view {
    use adw::subclass::prelude::*;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/overview-page.ui")]
        pub struct OverviewView {
            #[template_child]
            pub cards: TemplateChild<gtk::FlowBox>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for OverviewView {
            const NAME: &'static str = "CwOverviewPage";
            type Type = super::OverviewView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for OverviewView {}
        impl WidgetImpl for OverviewView {}
        impl BinImpl for OverviewView {}
    }

    glib::wrapper! {
        pub struct OverviewView(ObjectSubclass<imp::OverviewView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl OverviewView {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
