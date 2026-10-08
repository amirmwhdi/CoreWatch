//! Builds the main window from the module list and feeds it snapshots.
//!
//! The layout lives in `data/ui/stable/window.blp` (the `CwWindow` template).
//! This file never names a resource: it only loops over `modules::all()`.

use crate::modules::{self, ResourcePage};
use crate::{config, logging};
use adw::prelude::*;
use adw::subclass::prelude::*;
use corewatch_core::{Sampler, Snapshot, SysRoot};
use gtk::{gio, glib};
use std::cell::RefCell;
use std::rc::Rc;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(resource = "/io/github/amirmwhdi/Corewatch/ui/window.ui")]
    pub struct Window {
        #[template_child]
        pub split_view: TemplateChild<adw::NavigationSplitView>,
        #[template_child]
        pub sidebar: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub content_page: TemplateChild<adw::NavigationPage>,
        #[template_child]
        pub main_stack: TemplateChild<gtk::Stack>,
        #[template_child]
        pub stack: TemplateChild<gtk::Stack>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Window {
        const NAME: &'static str = "CwWindow";
        type Type = super::Window;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for Window {}
    impl WidgetImpl for Window {}
    impl WindowImpl for Window {}
    impl ApplicationWindowImpl for Window {}
    impl AdwApplicationWindowImpl for Window {}
}

glib::wrapper! {
    pub struct Window(ObjectSubclass<imp::Window>)
        @extends adw::ApplicationWindow, gtk::ApplicationWindow, gtk::Window, gtk::Widget,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable,
            gtk::ConstraintTarget, gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl Window {
    pub fn new(app: &adw::Application) -> Self {
        glib::Object::builder().property("application", app).build()
    }
}

pub fn present(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let root = SysRoot::from_env();
    logging::log_startup_banner(&root);
    let mut sampler = Sampler::new(root.clone(), config::SAMPLE_INTERVAL);

    let window = Window::new(app);
    let ui = window.imp();
    let stack = ui.stack.get();
    let sidebar = ui.sidebar.get();
    let split = ui.split_view.get();
    let content_page = ui.content_page.get();

    // Each module: collector into the sampler, page into the stack, row into the sidebar.
    let mut pages: Vec<(Box<dyn ResourcePage>, adw::ActionRow)> = Vec::new();
    for module in modules::all() {
        let page = (module.page)();
        match (module.collector)(&root) {
            Ok(collector) => {
                tracing::info!(module = page.id(), "module enabled");
                sampler.add(collector);
            }
            Err(error) => {
                tracing::warn!(
                    module = page.id(),
                    error.kind = error.kind(),
                    error = %error.chain(),
                    "module disabled"
                );
                continue;
            }
        }
        stack.add_named(&page.widget(), Some(page.id()));

        let row = adw::ActionRow::builder().title(page.title()).build();
        row.add_prefix(&gtk::Image::from_icon_name(page.icon_name()));
        row.set_widget_name(page.id());
        sidebar.append(&row);
        pages.push((page, row));
    }
    ui.main_stack
        .set_visible_child_name(if pages.is_empty() { "empty" } else { "pages" });

    sidebar.connect_row_selected(glib::clone!(
        #[weak]
        stack,
        #[weak]
        split,
        #[weak]
        content_page,
        move |_, row| {
            let Some(row) = row else { return };
            stack.set_visible_child_name(&row.widget_name());
            if let Some(action_row) = row.downcast_ref::<adw::ActionRow>() {
                content_page.set_title(&action_row.title());
            }
            split.set_show_content(true);
        }
    ));
    sidebar.select_row(sidebar.row_at_index(0).as_ref());

    // Start sampling and feed every page from the channel.
    let (tx, rx) = async_channel::bounded::<Snapshot>(1);
    let handle = match sampler.spawn(tx) {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(%error, "cannot start the sampler thread");
            window.present();
            return;
        }
    };
    let pages = Rc::new(pages);
    glib::spawn_future_local(async move {
        while let Ok(snapshot) = rx.recv().await {
            for (page, row) in pages.iter() {
                page.update(&snapshot);
                if let Some(text) = page.summary(&snapshot) {
                    row.set_subtitle(&text);
                }
            }
        }
    });

    // Closing the window drops the handle: the sampler stops, the channel
    // closes, and the update loop above ends.
    let handle = RefCell::new(Some(handle));
    window.connect_close_request(move |_| {
        handle.borrow_mut().take();
        glib::Propagation::Proceed
    });

    window.present();
}
