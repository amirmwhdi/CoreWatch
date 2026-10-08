//! Memory page: usage graph, used, available, cache and swap.

use super::{Module, ResourcePage};
use crate::config::HISTORY_LEN;
use crate::i18n::{self, gettext};
use crate::widgets::{layout, Graph};
use adw::prelude::*;
use corewatch_core::memory::MemoryCollector;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::glib;

fn collector(_root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(MemoryCollector))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(MemoryPage::new())
}

pub const MODULE: Module = Module { collector, page };

struct MemoryPage {
    root: gtk::Widget,
    graph: Graph,
    used: adw::ActionRow,
    available: adw::ActionRow,
    cached: adw::ActionRow,
    swap: adw::ActionRow,
    swap_bar: gtk::LevelBar,
}

impl MemoryPage {
    fn new() -> Self {
        let graph = Graph::new(HISTORY_LEN, 160);
        let row = |title: &str| adw::ActionRow::builder().title(title).subtitle("…").build();
        let (used, available, cached, swap) = (
            row(&gettext("Used")),
            row(&gettext("Available")),
            row(&gettext("Cache and buffers")),
            row(&gettext("Swap")),
        );

        let swap_bar = gtk::LevelBar::builder()
            .width_request(120)
            .valign(gtk::Align::Center)
            .build();
        swap.add_suffix(&swap_bar);

        let group = adw::PreferencesGroup::new();
        for r in [&used, &available, &cached, &swap] {
            group.add(r);
        }

        let content = layout::page_box();
        content.append(&graph);
        content.append(&group);

        Self {
            root: layout::scrolled(&content),
            graph,
            used,
            available,
            cached,
            swap,
            swap_bar,
        }
    }
}

impl ResourcePage for MemoryPage {
    fn id(&self) -> &'static str {
        "memory"
    }

    fn title(&self) -> String {
        gettext("Memory")
    }

    fn icon_name(&self) -> &'static str {
        "drive-multidisk-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        let Some(m) = &snapshot.memory else { return };
        let size = |bytes: u64| glib::format_size(bytes);

        self.graph.push(m.used_fraction());
        self.used.set_subtitle(&i18n::fmt(
            // Translators: e.g. "6.2 GB of 16 GB (39%)"
            &gettext("{used} of {total} ({percent})"),
            &[
                ("used", &size(m.used)),
                ("total", &size(m.total)),
                ("percent", &i18n::percent(m.used_fraction())),
            ],
        ));
        self.available.set_subtitle(&size(m.available));
        self.cached.set_subtitle(&i18n::fmt(
            &gettext("{cache} cache · {buffers} buffers"),
            &[("cache", &size(m.cached)), ("buffers", &size(m.buffers))],
        ));

        if m.swap_total == 0 {
            self.swap.set_subtitle(&gettext("No swap configured"));
            self.swap_bar.set_visible(false);
        } else {
            self.swap.set_subtitle(&i18n::fmt(
                &gettext("{used} of {total}"),
                &[("used", &size(m.swap_used)), ("total", &size(m.swap_total))],
            ));
            self.swap_bar.set_visible(true);
            self.swap_bar.set_value(f64::from(m.swap_fraction()));
        }
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        snapshot
            .memory
            .as_ref()
            .map(|m| i18n::percent(m.used_fraction()))
    }
}
