//! Preferences dialog for Corewatch.
//!
//! Every row saves at once and takes effect on the next tick: units through
//! `i18n::set_units`, the graph length through `widgets::graph::set_history`.

use crate::i18n::{self, gettext, NetworkUnits, SizeUnits, TemperatureUnit};
use crate::settings::{AppSettings, StartPage, ThemeMode, UpdateSpeed};
use adw::prelude::*;
use adw::subclass::prelude::*;
use corewatch_core::SamplerHandle;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/preferences.ui")]
    pub struct PreferencesDialog {
        #[template_child]
        pub theme_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub start_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub interval_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub temperature_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub sizes_row: TemplateChild<adw::ComboRow>,
        #[template_child]
        pub network_row: TemplateChild<adw::ComboRow>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PreferencesDialog {
        const NAME: &'static str = "CwPreferencesDialog";
        type Type = super::PreferencesDialog;
        type ParentType = adw::PreferencesDialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PreferencesDialog {}
    impl WidgetImpl for PreferencesDialog {}
    impl AdwDialogImpl for PreferencesDialog {}
    impl PreferencesDialogImpl for PreferencesDialog {}
}

glib::wrapper! {
    pub struct PreferencesDialog(ObjectSubclass<imp::PreferencesDialog>)
        @extends adw::PreferencesDialog, adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

/// Load the settings, change them with `f`, save, and apply what changed.
fn update(f: impl FnOnce(&mut AppSettings)) -> AppSettings {
    let mut settings = AppSettings::load();
    f(&mut settings);
    settings.save();
    settings
}

impl PreferencesDialog {
    /// `pages` are the (id, title) of the pages in the sidebar, in order.
    pub fn new(
        sampler_handle: Rc<RefCell<Option<SamplerHandle>>>,
        pages: &[(String, String)],
    ) -> Self {
        let dialog: Self = glib::Object::builder().build();
        let imp = dialog.imp();
        let settings = AppSettings::load();

        imp.theme_row.set_selected(settings.theme as u32);
        imp.theme_row.connect_selected_notify(|row| {
            update(|s| s.theme = ThemeMode::from_index(row.selected())).apply_theme();
        });

        // "Last Opened Page" first, then every page by its sidebar title.
        let start_ids: Vec<Option<String>> = std::iter::once(None)
            .chain(pages.iter().map(|(id, _)| Some(id.clone())))
            .collect();
        let titles: Vec<String> = std::iter::once(gettext("Last Opened Page"))
            .chain(pages.iter().map(|(_, title)| title.clone()))
            .collect();
        let titles: Vec<&str> = titles.iter().map(String::as_str).collect();
        imp.start_row
            .set_model(Some(&gtk::StringList::new(&titles)));
        let selected = match &settings.start_page {
            StartPage::Last => 0,
            StartPage::Page(id) => start_ids
                .iter()
                .position(|s| s.as_deref() == Some(id))
                .unwrap_or(0),
        };
        imp.start_row
            .set_selected(u32::try_from(selected).unwrap_or(0));
        imp.start_row.connect_selected_notify(move |row| {
            let choice = start_ids.get(row.selected() as usize).cloned().flatten();
            update(|s| {
                s.start_page = match choice {
                    Some(id) => StartPage::Page(id),
                    None => StartPage::Last,
                }
            });
        });

        imp.interval_row.set_selected(settings.speed as u32);
        imp.interval_row.connect_selected_notify(move |row| {
            let settings = update(|s| s.speed = UpdateSpeed::from_index(row.selected()));
            // The same minute at a new interval: a different number of points.
            crate::widgets::graph::set_history(settings.speed.history_points());
            if let Some(handle) = sampler_handle.borrow().as_ref() {
                handle.set_interval(settings.speed.as_duration());
            }
        });

        let units = settings.units;
        imp.temperature_row
            .set_selected(u32::from(units.temperature == TemperatureUnit::Fahrenheit));
        imp.temperature_row.connect_selected_notify(|row| {
            let settings = update(|s| {
                s.units.temperature = if row.selected() == 1 {
                    TemperatureUnit::Fahrenheit
                } else {
                    TemperatureUnit::Celsius
                }
            });
            i18n::set_units(settings.units);
        });

        imp.sizes_row
            .set_selected(u32::from(units.sizes == SizeUnits::Decimal));
        imp.sizes_row.connect_selected_notify(|row| {
            let settings = update(|s| {
                s.units.sizes = if row.selected() == 1 {
                    SizeUnits::Decimal
                } else {
                    SizeUnits::Binary
                }
            });
            i18n::set_units(settings.units);
        });

        imp.network_row
            .set_selected(u32::from(units.network == NetworkUnits::Bits));
        imp.network_row.connect_selected_notify(|row| {
            let settings = update(|s| {
                s.units.network = if row.selected() == 1 {
                    NetworkUnits::Bits
                } else {
                    NetworkUnits::Bytes
                }
            });
            i18n::set_units(settings.units);
        });

        dialog
    }
}
