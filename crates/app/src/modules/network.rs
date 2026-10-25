//! Network page: one block per interface with a download and upload graph,
//! speeds, addresses and link details.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use crate::widgets::{Graph, SensorGroup};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::network::{InterfaceKind, NetInterface, NetworkCollector};
use corewatch_core::sensors::Home;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use std::cell::RefCell;
use std::collections::BTreeMap;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(NetworkCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(NetworkPage::new())
}

pub const MODULE: Module = Module {
    collector: Some(collector),
    page,
};

/// The graph never zooms in further than this, so an idle link does not
/// look busy: 16 KiB/s.
pub const GRAPH_FLOOR: f32 = 16.0 * 1024.0;

struct NetworkPage {
    sensors: SensorGroup,
    root: gtk::Widget,
    interfaces_box: gtk::Box,
    no_interfaces: adw::StatusPage,
    /// Widgets per interface, keyed by kernel name; kept while the interface
    /// exists so its graph keeps its history.
    blocks: RefCell<BTreeMap<String, InterfaceBlock>>,
    /// Interface names in display order, to notice when the set changes.
    order: RefCell<Vec<String>>,
}

impl NetworkPage {
    /// The layout comes from `network-page.blp`; interface blocks are added
    /// at run time.
    fn new() -> Self {
        let view = view::NetworkView::new();
        let ui = view.imp();
        Self {
            interfaces_box: ui.interfaces_box.get(),
            no_interfaces: ui.no_interfaces.get(),
            blocks: RefCell::default(),
            order: RefCell::default(),
            sensors: SensorGroup::new(ui.sensors_group.get(), Home::Network),
            root: view.upcast(),
        }
    }

    /// Add blocks for new interfaces and drop blocks for removed ones;
    /// blocks of interfaces that stay keep their widgets.
    fn sync(&self, interfaces: &[NetInterface]) {
        let mut blocks = self.blocks.borrow_mut();
        let mut order = self.order.borrow_mut();
        if !order.iter().eq(interfaces.iter().map(|i| &i.name)) {
            blocks.retain(|name, block| {
                let keep = interfaces.iter().any(|i| &i.name == name);
                if !keep {
                    self.interfaces_box.remove(&block.container);
                }
                keep
            });
            // Re-append in the collector's order: physical first, then VPNs.
            for interface in interfaces {
                let block = blocks
                    .entry(interface.name.clone())
                    .or_insert_with(|| InterfaceBlock::new(interface));
                if block.container.parent().is_some() {
                    self.interfaces_box.remove(&block.container);
                }
                self.interfaces_box.append(&block.container);
            }
            *order = interfaces.iter().map(|i| i.name.clone()).collect();
            self.no_interfaces.set_visible(interfaces.is_empty());
        }
        for interface in interfaces {
            if let Some(block) = blocks.get(&interface.name) {
                block.update(interface);
            }
        }
    }
}

impl ResourcePage for NetworkPage {
    fn id(&self) -> &'static str {
        "network"
    }

    fn title(&self) -> String {
        gettext("Network")
    }

    fn icon_name(&self) -> &'static str {
        "network-wireless-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        self.sensors.update(snapshot.sensors.as_ref());
        if let Some(network) = &snapshot.network {
            self.sync(&network.interfaces);
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.network.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        // Download and upload together, like the Disks row: short enough
        // for the sidebar.
        snapshot
            .network
            .as_ref()
            .map(|n| i18n::net_speed(n.rx_bps().saturating_add(n.tx_bps())))
    }
}

/// Heading, traffic graph and detail rows of one interface.
struct InterfaceBlock {
    container: gtk::Box,
    description: gtk::Label,
    graph: Graph,
    download: adw::ActionRow,
    upload: adw::ActionRow,
    ipv4: adw::ActionRow,
    ipv6: adw::ActionRow,
}

impl InterfaceBlock {
    fn new(interface: &NetInterface) -> Self {
        let title = gtk::Label::builder()
            .label(kind_label(interface.kind))
            .xalign(0.0)
            .css_classes(["heading"])
            .build();
        let description = gtk::Label::builder()
            .label(interface.name.as_str())
            .xalign(0.0)
            .css_classes(["dim-label", "caption"])
            .build();

        let graph = Graph::new_autoscale(100, GRAPH_FLOOR);
        graph.update_property(&[gtk::accessible::Property::Label(&i18n::fmt(
            &gettext("Traffic of {name}"),
            &[("name", &interface.name)],
        ))]);

        let row = |title: &str| {
            adw::ActionRow::builder()
                .title(title)
                .subtitle("…")
                .subtitle_selectable(true)
                .build()
        };
        let (download, upload) = (row(&gettext("Download")), row(&gettext("Upload")));
        download.add_prefix(&legend_dot("primary"));
        upload.add_prefix(&legend_dot("secondary"));
        let (ipv4, ipv6) = (row(&gettext("IPv4 address")), row(&gettext("IPv6 address")));
        let rows = adw::PreferencesGroup::new();
        rows.add(&download);
        rows.add(&upload);
        rows.add(&ipv4);
        rows.add(&ipv6);
        if let Some(mac) = &interface.mac {
            let hardware = row(&gettext("Hardware address"));
            hardware.set_subtitle(mac);
            rows.add(&hardware);
        }

        let header = gtk::Box::new(gtk::Orientation::Vertical, 2);
        header.append(&title);
        header.append(&description);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 12);
        container.append(&header);
        container.append(&graph);
        container.append(&rows);

        Self {
            container,
            description,
            graph,
            download,
            upload,
            ipv4,
            ipv6,
        }
    }

    fn update(&self, interface: &NetInterface) {
        if let (Some(rx), Some(tx)) = (interface.rx_bps, interface.tx_bps) {
            self.graph.push_pair(rx as f32, tx as f32);
        }
        self.description.set_label(&details(interface));
        self.download.set_subtitle(&transfer(
            interface.rx_bps,
            &i18n::fmt(
                &gettext("{total} received"),
                &[("total", &i18n::size(interface.rx_total))],
            ),
        ));
        self.upload.set_subtitle(&transfer(
            interface.tx_bps,
            &i18n::fmt(
                &gettext("{total} sent"),
                &[("total", &i18n::size(interface.tx_total))],
            ),
        ));

        let join = |ips: Vec<String>| ips.join("\n");
        let v4 = join(
            interface
                .addresses
                .ipv4
                .iter()
                .map(ToString::to_string)
                .collect(),
        );
        let v6 = join(
            interface
                .addresses
                .ipv6
                .iter()
                .map(ToString::to_string)
                .collect(),
        );
        set_optional(&self.ipv4, &v4);
        set_optional(&self.ipv6, &v6);
    }
}

/// Set a row's text only when it changed, and hide the row when empty.
fn set_optional(row: &adw::ActionRow, text: &str) {
    row.set_visible(!text.is_empty());
    if row.subtitle().as_deref() != Some(text) {
        row.set_subtitle(text);
    }
}

/// "wlp2s0 · Signal 80%", "enp3s0 · 1 Gb/s", "eth0 · Disconnected".
fn details(interface: &NetInterface) -> String {
    let state = if !interface.connected {
        gettext("Disconnected")
    } else if let Some(signal) = interface.signal {
        i18n::fmt(
            &gettext("Signal {percent}"),
            &[("percent", &i18n::percent(signal))],
        )
    } else if let Some(mbps) = interface.link_mbps {
        link_speed(mbps)
    } else {
        gettext("Connected")
    };
    i18n::fmt(
        // Translators: interface name and state, e.g. "wlp2s0 · Signal 80%"
        &gettext("{name} · {state}"),
        &[("name", &interface.name), ("state", &state)],
    )
}

/// Link speeds in the units on the box: "100 Mb/s", "2.5 Gb/s".
fn link_speed(mbps: u32) -> String {
    if mbps >= 1000 {
        let gbps = format!("{}", f64::from(mbps) / 1000.0);
        // Translators: a network link speed in gigabits, e.g. "2.5 Gb/s"
        i18n::fmt(&gettext("{value} Gb/s"), &[("value", &gbps)])
    } else {
        // Translators: a network link speed in megabits, e.g. "100 Mb/s"
        i18n::fmt(&gettext("{value} Mb/s"), &[("value", &mbps)])
    }
}

/// "1.2 MiB/s · 3.4 GiB received"; "…" while the speed is not known yet.
fn transfer(bps: Option<u64>, total: &str) -> String {
    match bps {
        Some(bps) => i18n::fmt(
            &gettext("{speed} · {total}"),
            &[("speed", &i18n::net_speed(bps)), ("total", &total)],
        ),
        None => "…".to_owned(),
    }
}

fn kind_label(kind: InterfaceKind) -> String {
    match kind {
        InterfaceKind::Ethernet => gettext("Ethernet"),
        InterfaceKind::WiFi => gettext("Wi-Fi"),
        InterfaceKind::Vpn => gettext("VPN"),
        InterfaceKind::Other => gettext("Network"),
    }
}

/// The small colored dot that ties a row to its line in the graph.
pub fn legend_dot(series: &str) -> gtk::Widget {
    gtk::Box::builder()
        .valign(gtk::Align::Center)
        .css_classes(["cw-dot", series])
        .accessible_role(gtk::AccessibleRole::Presentation)
        .build()
        .upcast()
}

/// The `CwNetworkPage` template from `data/ui/stable/network-page.blp`.
mod view {
    use adw::subclass::prelude::*;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/network-page.ui")]
        pub struct NetworkView {
            #[template_child]
            pub sensors_group: TemplateChild<adw::PreferencesGroup>,
            #[template_child]
            pub interfaces_box: TemplateChild<gtk::Box>,
            #[template_child]
            pub no_interfaces: TemplateChild<adw::StatusPage>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for NetworkView {
            const NAME: &'static str = "CwNetworkPage";
            type Type = super::NetworkView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for NetworkView {}
        impl WidgetImpl for NetworkView {}
        impl BinImpl for NetworkView {}
    }

    glib::wrapper! {
        pub struct NetworkView(ObjectSubclass<imp::NetworkView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl NetworkView {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
