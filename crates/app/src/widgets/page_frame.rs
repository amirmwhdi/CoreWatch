//! `PageFrame`: a page plus the "Data unavailable" banner above it.
//!
//! The layout lives in `data/ui/stable/page-frame.blp`. The frame counts the
//! ticks in a row without data and reveals the banner after
//! [`MISSES_BEFORE_BANNER`], so a single slow tick never makes it flicker.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{glib, CompositeTemplate, TemplateChild};
use std::cell::Cell;

/// Ticks in a row without data before the banner shows.
pub const MISSES_BEFORE_BANNER: u32 = 3;

mod imp {
    use super::*;

    #[derive(Default, CompositeTemplate)]
    #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/page-frame.ui")]
    pub struct PageFrame {
        #[template_child]
        pub banner: TemplateChild<adw::Banner>,
        #[template_child]
        pub content: TemplateChild<adw::Bin>,
        pub misses: Cell<u32>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PageFrame {
        const NAME: &'static str = "CwPageFrame";
        type Type = super::PageFrame;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PageFrame {}
    impl WidgetImpl for PageFrame {}
    impl BoxImpl for PageFrame {}
}

glib::wrapper! {
    pub struct PageFrame(ObjectSubclass<imp::PageFrame>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl PageFrame {
    pub fn new(page: &impl IsA<gtk::Widget>) -> Self {
        let frame: Self = glib::Object::builder().build();
        frame.imp().content.set_child(Some(page));
        frame
    }

    /// Record one tick. Returns `true` while the page counts as unavailable.
    pub fn set_has_data(&self, has_data: bool) -> bool {
        let imp = self.imp();
        let misses = if has_data {
            0
        } else {
            imp.misses.get().saturating_add(1)
        };
        imp.misses.set(misses);
        let unavailable = misses >= MISSES_BEFORE_BANNER;
        if imp.banner.is_revealed() != unavailable {
            imp.banner.set_revealed(unavailable);
        }
        unavailable
    }
}
