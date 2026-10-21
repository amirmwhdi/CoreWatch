//! Preferences dialog for Corewatch.

use crate::settings::{AppSettings, ThemeMode, UpdateSpeed};
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
        pub interval_row: TemplateChild<adw::ComboRow>,
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

impl PreferencesDialog {
    pub fn new(sampler_handle: Rc<RefCell<Option<SamplerHandle>>>) -> Self {
        let dialog: Self = glib::Object::builder().build();
        let imp = dialog.imp();
        let settings = AppSettings::load();

        // Set initial values
        imp.theme_row.set_selected(settings.theme as u32);
        imp.interval_row.set_selected(settings.speed as u32);

        // Connect theme change
        imp.theme_row.connect_selected_notify(move |row| {
            let mut settings = AppSettings::load();
            settings.theme = ThemeMode::from_index(row.selected());
            settings.apply_theme();
            settings.save();
        });

        // Connect interval change
        imp.interval_row.connect_selected_notify(move |row| {
            let mut settings = AppSettings::load();
            settings.speed = UpdateSpeed::from_index(row.selected());
            settings.save();
            if let Some(handle) = sampler_handle.borrow().as_ref() {
                handle.set_interval(settings.speed.as_duration());
            }
        });

        dialog
    }
}
