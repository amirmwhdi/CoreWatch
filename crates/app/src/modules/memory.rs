//! Memory page: usage graph, used, available, cache and swap.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use crate::widgets::Graph;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
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
    /// The layout comes from `memory-page.blp`; keep handles to the parts
    /// that change.
    fn new() -> Self {
        let view = view::MemoryView::new();
        let ui = view.imp();
        Self {
            graph: ui.graph.get(),
            used: ui.used.get(),
            available: ui.available.get(),
            cached: ui.cached.get(),
            swap: ui.swap.get(),
            swap_bar: ui.swap_bar.get(),
            root: view.upcast(),
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

/// The `CwMemoryPage` template from `data/ui/stable/memory-page.blp`.
mod view {
    use crate::widgets::Graph;
    use adw::subclass::prelude::*;
    use gtk::prelude::StaticTypeExt;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/memory-page.ui")]
        pub struct MemoryView {
            #[template_child]
            pub graph: TemplateChild<Graph>,
            #[template_child]
            pub used: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub available: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub cached: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub swap: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub swap_bar: TemplateChild<gtk::LevelBar>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for MemoryView {
            const NAME: &'static str = "CwMemoryPage";
            type Type = super::MemoryView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                Graph::ensure_type();
                klass.bind_template();
            }

            fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
                obj.init_template();
            }
        }

        impl ObjectImpl for MemoryView {}
        impl WidgetImpl for MemoryView {}
        impl BinImpl for MemoryView {}
    }

    glib::wrapper! {
        pub struct MemoryView(ObjectSubclass<imp::MemoryView>)
            @extends adw::Bin, gtk::Widget,
            @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
    }

    impl MemoryView {
        pub fn new() -> Self {
            glib::Object::new()
        }
    }
}
