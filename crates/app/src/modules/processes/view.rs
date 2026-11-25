//! The `CwProcessesPage` template from `data/ui/stable/processes-page.blp`.

use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate, TemplateChild};

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/processes-page.ui")]
    pub struct ProcessesView {
        #[template_child]
        pub toasts: TemplateChild<adw::ToastOverlay>,
        #[template_child]
        pub search: TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub mine: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub group: TemplateChild<gtk::ToggleButton>,
        #[template_child]
        pub details_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub end_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub columns: TemplateChild<gtk::ColumnView>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ProcessesView {
        const NAME: &'static str = "CwProcessesPage";
        type Type = super::ProcessesView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ProcessesView {}
    impl WidgetImpl for ProcessesView {}
    impl BinImpl for ProcessesView {}
}

glib::wrapper! {
    pub struct ProcessesView(ObjectSubclass<imp::ProcessesView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ProcessesView {
    pub fn new() -> Self {
        glib::Object::new()
    }
}
