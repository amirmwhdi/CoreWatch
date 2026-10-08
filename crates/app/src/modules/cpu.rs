//! CPU page: total usage graph, model, frequency, one small graph per core.

use super::{Module, ResourcePage};
use crate::config::HISTORY_LEN;
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
        let (model, usage, freq) = (row("Model"), row("Usage"), row("Frequency"));

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
            .label("Logical cores")
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

    fn title(&self) -> &'static str {
        "Processor"
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
        let threads = match cpu.info.physical {
            Some(physical) => format!("{} cores, {} threads", physical, cpu.info.logical),
            None => format!("{} threads", cpu.info.logical),
        };
        self.usage
            .set_subtitle(&format!("{:.0}% · {threads}", cpu.usage * 100.0));
        self.freq.set_subtitle(&match cpu.max_freq_mhz() {
            Some(mhz) => format!("{:.2} GHz (fastest core)", f64::from(mhz) / 1000.0),
            None => "Unknown".to_owned(),
        });

        self.ensure_cores(cpu.cores.len());
        for ((graph, label), core) in self.cores.borrow().iter().zip(&cpu.cores) {
            graph.push(core.usage);
            label.set_label(&format!("CPU {} · {:.0}%", core.id, core.usage * 100.0));
        }
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        snapshot
            .cpu
            .as_ref()
            .map(|c| format!("{:.0}%", c.usage * 100.0))
    }
}
