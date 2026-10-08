//! CPU page: total usage graph, model, frequency, one small graph per core.

use super::{Module, ResourcePage};
use crate::config::HISTORY_LEN;
use crate::i18n::{self, gettext, ngettext};
use crate::widgets::{layout, Graph};
use adw::prelude::*;
use corewatch_core::cpu::CpuCollector;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use std::cell::{Cell, RefCell};

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(CpuCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(CpuPage::new())
}

pub const MODULE: Module = Module { collector, page };

struct CpuPage {
    root: gtk::Widget,
    total: Graph,
    model: adw::ActionRow,
    usage: adw::ActionRow,
    freq: adw::ActionRow,
    info_set: Cell<bool>,
    core_grid: gtk::FlowBox,
    cores: RefCell<Vec<(Graph, gtk::Label)>>,
}

impl CpuPage {
    fn new() -> Self {
        let total = Graph::new(HISTORY_LEN, 160);
        let row = |title: &str| {
            adw::ActionRow::builder()
                .title(title)
                .subtitle("…")
                .subtitle_selectable(true)
                .build()
        };
        let (model, usage, freq) = (
            row(&gettext("Model")),
            row(&gettext("Usage")),
            row(&gettext("Frequency")),
        );

        let info = adw::PreferencesGroup::new();
        info.add(&model);
        info.add(&usage);
        info.add(&freq);

        let core_grid = gtk::FlowBox::builder()
            .selection_mode(gtk::SelectionMode::None)
            .homogeneous(true)
            .min_children_per_line(2)
            .max_children_per_line(8)
            .row_spacing(12)
            .column_spacing(12)
            .build();
        let cores_title = gtk::Label::builder()
            .label(gettext("Logical cores"))
            .xalign(0.0)
            .css_classes(["heading"])
            .build();

        let content = layout::page_box();
        content.append(&total);
        content.append(&info);
        content.append(&cores_title);
        content.append(&core_grid);

        Self {
            root: layout::scrolled(&content),
            total,
            model,
            usage,
            freq,
            info_set: Cell::new(false),
            core_grid,
            cores: RefCell::default(),
        }
    }

    /// The core count is known only after the first sample; build small graphs
    /// on demand.
    fn ensure_cores(&self, n: usize) {
        let mut cores = self.cores.borrow_mut();
        while cores.len() < n {
            let graph = Graph::new(HISTORY_LEN, 48);
            let label = gtk::Label::builder()
                .xalign(0.0)
                .css_classes(["caption", "numeric"])
                .build();
            let cell = gtk::Box::new(gtk::Orientation::Vertical, 4);
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
        let Some(cpu) = &snapshot.cpu else { return };

        self.total.push(cpu.usage);

        if !self.info_set.get() {
            self.model.set_subtitle(&cpu.info.model);
            self.info_set.set(true);
        }
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
        self.usage.set_subtitle(&i18n::fmt(
            // Translators: e.g. "37% · 8 cores, 16 threads"
            &gettext("{usage} · {threads}"),
            &[("usage", &i18n::percent(cpu.usage)), ("threads", &threads)],
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

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        snapshot.cpu.as_ref().map(|c| i18n::percent(c.usage))
    }
}
