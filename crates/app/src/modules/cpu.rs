//! CPU page: total usage graph, model, frequency, one small graph per core.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext, ngettext};
use crate::widgets::{Graph, SensorGroup};
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::cpu::CpuCollector;
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
        gettext("Processor")
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
            let threads = i18n::fmt(
                &ngettext("{n} thread", "{n} threads", i18n::count(logical)),
                &[("n", &logical)],
            );
            let threads = match cpu.info.physical {
                Some(physical) => {
                    let cores = i18n::fmt(
                        &ngettext("{n} core", "{n} cores", i18n::count(physical)),
                        &[("n", &physical)],
                    );
                    // Translators: e.g. "8 cores, 16 threads"
                    i18n::fmt(
                        &gettext("{cores}, {threads}"),
                        &[("cores", &cores), ("threads", &threads)],
                    )
                }
                None => threads,
            };
            *self.threads_desc.borrow_mut() = threads;
            self.info_set.set(true);
        }
        let threads = self.threads_desc.borrow();
        self.usage.set_subtitle(&i18n::fmt(
            // Translators: e.g. "37% · 8 cores, 16 threads"
            &gettext("{usage} · {threads}"),
            &[("usage", &i18n::percent(cpu.usage)), ("threads", &*threads)],
        ));
        self.freq.set_subtitle(&match cpu.max_freq_mhz() {
            Some(mhz) => i18n::fmt(
                &gettext("{ghz} GHz (fastest core)"),
                &[("ghz", &format!("{:.2}", f64::from(mhz) / 1000.0))],
            ),
            None => gettext("Unknown"),
        });

        self.ensure_cores(cpu.cores.len());
        for ((graph, label), core) in self.cores.borrow().iter().zip(&cpu.cores) {
            graph.push(core.usage);
            label.set_label(&i18n::fmt(
                // Translators: one logical core, e.g. "CPU 3 · 12%"
                &gettext("CPU {id} · {usage}"),
                &[("id", &core.id), ("usage", &i18n::percent(core.usage))],
            ));
        }
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.cpu.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        snapshot.cpu.as_ref().map(|c| i18n::percent(c.usage))
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
