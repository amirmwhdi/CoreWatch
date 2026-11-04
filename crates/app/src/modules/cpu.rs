//! CPU page: total usage graph, model, frequency, one small graph per core.

use super::{Module, ResourcePage};
use crate::format;
use crate::widgets::{Graph, SensorGroup};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::cpu::{CoreKind, CpuCollector};
use corewatch_core::sensors::Home;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use std::cell::{Cell, RefCell};

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(CpuCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(CpuPage::new())
}

pub const MODULE: Module = Module {
    collector: Some(collector),
    page,
};

struct CpuPage {
    sensors: SensorGroup,
    root: gtk::Widget,
    total: Graph,
    model: adw::ActionRow,
    usage: adw::ActionRow,
    freq: adw::ActionRow,
    pressure: adw::ActionRow,
    info_set: Cell<bool>,
    threads_desc: RefCell<String>,
    core_grid: gtk::FlowBox,
    cores: RefCell<Vec<(Graph, gtk::Label)>>,
}

impl CpuPage {
    /// The layout comes from `cpu-page.blp`; keep handles to the parts that
    /// change.
    fn new() -> Self {
        let view = view::CpuView::new();
        let ui = view.imp();
        Self {
            total: ui.total.get(),
            model: ui.model.get(),
            usage: ui.usage.get(),
            freq: ui.freq.get(),
            pressure: ui.pressure.get(),
            info_set: Cell::new(false),
            threads_desc: RefCell::default(),
            core_grid: ui.core_grid.get(),
            cores: RefCell::default(),
            sensors: SensorGroup::new(ui.sensors_group.get(), Home::Cpu),
            root: view.upcast(),
        }
    }

    /// The core count is known only after the first sample; build small graphs
    /// on demand.
    fn ensure_cores(&self, n: usize) {
        let mut cores = self.cores.borrow_mut();
        while cores.len() < n {
            let graph = Graph::new(48);
            graph.set_width_request(96);
            let label = gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["caption", "numeric"])
                .build();
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 4);
            cell.set_width_request(96);
            cell.append(&label);
            cell.append(&graph);
            self.core_grid.append(&cell);
            cores.push((graph, label));
        }
    }
}

impl ResourcePage for CpuPage {
    fn id(&self) -> &'static str {
        "cpu"
    }

    fn title(&self) -> String {
        String::from("Processor")
    }

    fn icon_name(&self) -> &'static str {
        "computer-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        self.sensors.update(snapshot.sensors.as_ref());
        let Some(cpu) = &snapshot.cpu else { return };

        self.total.push(cpu.usage);

        if !self.info_set.get() {
            self.model.set_subtitle(&cpu.info.model);
            let logical = cpu.info.logical;
            let threads = format::fmt(
                format::plural(logical, "{n} thread", "{n} threads"),
                &[("n", &logical)],
            );
            let topology = &cpu.topology;
            let threads = match cpu.info.physical {
                // e.g. "6 performance and 8 efficient cores, 20 threads"
                _ if topology.is_hybrid() => format::fmt(
                    "{p} performance and {e} efficient cores, {threads}",
                    &[
                        ("p", &topology.performance_cores),
                        ("e", &topology.efficient_cores),
                        ("threads", &threads),
                    ],
                ),
                Some(physical) => {
                    let cores = format::fmt(
                        format::plural(physical, "{n} core", "{n} cores"),
                        &[("n", &physical)],
                    );
                    // e.g. "8 cores, 16 threads"
                    format::fmt(
                        "{cores}, {threads}",
                        &[("cores", &cores), ("threads", &threads)],
                    )
                }
                None => threads,
            };
            *self.threads_desc.borrow_mut() = threads;
            self.info_set.set(true);
        }
        let threads = self.threads_desc.borrow();
        self.usage.set_subtitle(&format::fmt(
            // e.g. "37% · 8 cores, 16 threads"
            "{usage} · {threads}",
            &[
                ("usage", &format::percent(cpu.usage)),
                ("threads", &*threads),
            ],
        ));
        self.freq.set_subtitle(&match cpu.max_freq_mhz() {
            Some(mhz) => format::fmt(
                "{ghz} GHz (fastest core)",
                &[("ghz", &format!("{:.2}", f64::from(mhz) / 1000.0))],
            ),
            None => String::from("Unknown"),
        });

        // The system-wide "full" line of the CPU is always zero: some only.
        format::set_pressure_row(&self.pressure, cpu.pressure.as_ref(), false);

        self.ensure_cores(cpu.cores.len());
        for ((graph, label), core) in self.cores.borrow().iter().zip(&cpu.cores) {
            graph.push(core.usage);
            let usage = format::percent(core.usage);
            label.set_label(&match core.kind {
                // one logical core, e.g. "CPU 3 · 12%", on a hybrid
                // processor "CPU 3 · P · 12%"
                None => format::fmt("CPU {id} · {usage}", &[("id", &core.id), ("usage", &usage)]),
                Some(kind) => format::fmt(
                    "CPU {id} · {kind} · {usage}",
                    &[
                        ("id", &core.id),
                        ("kind", &kind_letter(kind)),
                        ("usage", &usage),
                    ],
                ),
            });
            if let (Some(kind), true) = (core.kind, label.tooltip_text().is_none()) {
                label.set_tooltip_text(Some(match kind {
                    CoreKind::Performance => "Performance core",
                    CoreKind::Efficient => "Efficient core",
                }));
            }
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.cpu.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        snapshot.cpu.as_ref().map(|c| format::percent(c.usage))
    }
}

/// "P" for a performance core, "E" for an efficient one.
fn kind_letter(kind: CoreKind) -> &'static str {
    match kind {
        CoreKind::Performance => "P",
        CoreKind::Efficient => "E",
    }
}

/// The `CwCpuPage` template from `data/ui/stable/cpu-page.blp`.
mod view {
    use crate::widgets::Graph;
    use adw::subclass::prelude::*;
    use gtk::prelude::StaticTypeExt;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/cpu-page.ui")]
        pub struct CpuView {
            #[template_child]
            pub sensors_group: TemplateChild<adw::PreferencesGroup>,
            #[template_child]
            pub total: TemplateChild<Graph>,
            #[template_child]
            pub model: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub usage: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub freq: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub pressure: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub core_grid: TemplateChild<gtk::FlowBox>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for CpuView {
            const NAME: &'static str = "CwCpuPage";
            type Type = super::CpuView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                Graph::ensure_type();
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for CpuView {}
        impl WidgetImpl for CpuView {}
        impl BinImpl for CpuView {}
    }

    glib::wrapper! {
        pub struct CpuView(ObjectSubclass<imp::CpuView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl CpuView {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
