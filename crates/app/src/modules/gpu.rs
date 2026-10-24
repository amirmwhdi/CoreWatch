//! GPU page: usage graph, graphics card model, driver, clock speed, VRAM, and temperature.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext};
use crate::widgets::Graph;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::gpu::GpuCollector;
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::glib;
use std::cell::Cell;

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(GpuCollector::new(root)?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(GpuPage::new())
}

pub const MODULE: Module = Module { collector, page };

struct GpuPage {
    root: gtk::Widget,
    graph: Graph,
    model: adw::ActionRow,
    driver: adw::ActionRow,
    usage: adw::ActionRow,
    freq: adw::ActionRow,
    vram: adw::ActionRow,
    vram_bar: gtk::LevelBar,
    temp: adw::ActionRow,
    static_info_set: Cell<bool>,
}

impl GpuPage {
    fn new() -> Self {
        let view = view::GpuView::new();
        let ui = view.imp();
        Self {
            graph: ui.graph.get(),
            model: ui.model.get(),
            driver: ui.driver.get(),
            usage: ui.usage_row.get(),
            freq: ui.freq.get(),
            vram: ui.vram.get(),
            vram_bar: ui.vram_bar.get(),
            temp: ui.temp.get(),
            static_info_set: Cell::new(false),
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
        let Some(gpu) = &snapshot.gpu else { return };
        let Some(first) = gpu.devices.first() else {
            return;
        };

        // amdgpu reports how busy the GPU is; i915/xe only report the clock,
        // so the graph shows the clock share there instead of a made-up usage.
        if let Some(value) = first.usage.or_else(|| first.freq_fraction()) {
            self.graph.push(value);
        }

        if !self.static_info_set.get() {
            self.model.set_subtitle(&first.name);
            self.driver
                .set_subtitle(first.driver.as_deref().unwrap_or(&gettext("Unknown")));
            self.static_info_set.set(true);
        }

        self.usage.set_subtitle(&match first.usage {
            Some(usage) => i18n::percent(usage),
            None => gettext("Not reported by the driver"),
        });

        match (first.freq_mhz, first.max_freq_mhz) {
            (Some(cur), Some(max)) => {
                self.freq.set_subtitle(&i18n::fmt(
                    &gettext("{cur} MHz / {max} MHz"),
                    &[("cur", &cur), ("max", &max)],
                ));
            }
            (Some(cur), None) => {
                self.freq
                    .set_subtitle(&i18n::fmt(&gettext("{cur} MHz"), &[("cur", &cur)]));
            }
            _ => {
                self.freq.set_subtitle(&gettext("Unknown"));
            }
        }

        match (first.vram_used, first.vram_total) {
            (Some(used), Some(total)) => {
                self.vram.set_subtitle(&i18n::fmt(
                    &gettext("{used} of {total}"),
                    &[
                        (
                            "used",
                            &glib::format_size_full(used, glib::FormatSizeFlags::IEC_UNITS),
                        ),
                        (
                            "total",
                            &glib::format_size_full(total, glib::FormatSizeFlags::IEC_UNITS),
                        ),
                    ],
                ));
                self.vram_bar.set_visible(true);
                self.vram_bar
                    .set_value(f64::from(first.vram_fraction().unwrap_or(0.0)));
            }
            _ => {
                self.vram.set_subtitle(&gettext("Shared / System RAM"));
                self.vram_bar.set_visible(false);
            }
        }

        match first.temp_c {
            Some(temp) => {
                self.temp
                    .set_subtitle(&i18n::fmt(&gettext("{t} °C"), &[("t", &temp)]));
                self.temp.set_visible(true);
            }
            None => {
                self.temp.set_visible(false);
            }
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
        let mhz = gpu.devices.first()?.freq_mhz?;
        Some(i18n::fmt(&gettext("{cur} MHz"), &[("cur", &mhz)]))
    }
}

/// The `CwGpuPage` template from `data/ui/stable/gpu-page.blp`.
mod view {
    use crate::widgets::Graph;
    use adw::subclass::prelude::*;
    use gtk::prelude::StaticTypeExt;
    use gtk::{glib, CompositeTemplate, TemplateChild};

    mod imp {
        use super::*;

        #[derive(Default, CompositeTemplate)]
        #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/gpu-page.ui")]
        pub struct GpuView {
            #[template_child]
            pub graph: TemplateChild<Graph>,
            #[template_child]
            pub model: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub driver: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub usage_row: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub freq: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub vram: TemplateChild<adw::ActionRow>,
            #[template_child]
            pub vram_bar: TemplateChild<gtk::LevelBar>,
            #[template_child]
            pub temp: TemplateChild<adw::ActionRow>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for GpuView {
            const NAME: &'static str = "CwGpuPage";
            type Type = super::GpuView;
            type ParentType = adw::Bin;

            fn class_init(klass: &mut Self::Class) {
                Graph::ensure_type();
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
