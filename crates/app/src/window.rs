//! Builds the main window from the module list and feeds it snapshots.
//!
//! The layout lives in `data/ui/stable/window.blp` (the `CwWindow` template).
//! This file never names a resource: it only loops over `modules::all()`.

use crate::i18n::gettext;
use crate::modules::{self, ResourcePage};
use crate::widgets::PageFrame;
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
    let settings = crate::settings::AppSettings::load();
    settings.apply_theme();

    if let Some(display) = gtk::gdk::Display::default() {
        let theme = gtk::IconTheme::for_display(&display);
        theme.add_resource_path("/io/github/amirmwhdi/Corewatch/icons");
    }
    gtk::Window::set_default_icon_name(config::APP_ID);

    let mut sampler = Sampler::new(root.clone(), settings.speed.as_duration());

    let window = Window::new(app);
    let ui = window.imp();
    let stack = ui.stack.get();
    let sidebar = ui.sidebar.get();
    let split = ui.split_view.get();
    let content_page = ui.content_page.get();

    // Each module: collector into the sampler, page into the stack, row into the sidebar.
    let mut pages: Vec<(Box<dyn ResourcePage>, PageFrame, adw::ActionRow)> = Vec::new();
    for module in modules::all() {
        let page = (module.page)();
        match module
            .collector
            .map_or(Ok(None), |new| new(&root).map(Some))
        {
            Ok(None) => {}
            Ok(Some(collector)) => {
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
        let frame = PageFrame::new(&page.widget());
        stack.add_named(&frame, Some(page.id()));

        let row = adw::ActionRow::builder().title(page.title()).build();
        row.add_prefix(&gtk::Image::from_icon_name(page.icon_name()));
        row.set_widget_name(page.id());
        sidebar.append(&row);
        pages.push((page, frame, row));
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
    let handle = Rc::new(RefCell::new(Some(handle)));

    // Actions for main menu
    let pref_action = gio::SimpleAction::new("preferences", None);
    pref_action.connect_activate(glib::clone!(
        #[weak]
        window,
        #[strong]
        handle,
        move |_, _| {
            let dialog = crate::preferences::PreferencesDialog::new(handle.clone());
            dialog.present(Some(&window));
        }
    ));
    app.add_action(&pref_action);
    app.set_accels_for_action("app.preferences", &["<primary>comma"]);

    let about_action = gio::SimpleAction::new("about", None);
    about_action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            let about = adw::AboutDialog::builder()
                .application_name("Corewatch")
                .application_icon(config::APP_ID)
                .version(config::VERSION)
                .copyright("© 2026 Amirmahdi")
                .license_type(gtk::License::Gpl30)
                .developer_name("Amirmahdi")
                .website("https://github.com/amirmwhdi/corewatch")
                .issue_url("https://github.com/amirmwhdi/corewatch/issues")
                .build();
            add_whats_new(&about);
            about.present(Some(&window));
        }
    ));
    app.add_action(&about_action);

    // The banner's "Details" button: open the folder with the log file.
    let log_action = gio::SimpleAction::new("show-log", None);
    log_action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| {
            let dir = gio::File::for_path(logging::log_dir());
            gtk::FileLauncher::new(Some(&dir)).launch(
                Some(&window),
                gio::Cancellable::NONE,
                |result| {
                    if let Err(error) = result {
                        tracing::warn!(%error, "cannot open the log folder");
                    }
                },
            );
        }
    ));
    app.add_action(&log_action);

    // Overview cards open their page: `app.show-page("cpu")`.
    let show_page = gio::SimpleAction::new("show-page", Some(glib::VariantTy::STRING));
    show_page.connect_activate(glib::clone!(
        #[weak]
        sidebar,
        move |_, param| {
            let Some(id) = param.and_then(|p| p.str()) else {
                return;
            };
            let mut index = 0;
            while let Some(row) = sidebar.row_at_index(index) {
                if row.widget_name() == id {
                    sidebar.select_row(Some(&row));
                    return;
                }
                index += 1;
            }
        }
    ));
    app.add_action(&show_page);

    let pages = Rc::new(pages);
    glib::spawn_future_local(async move {
        while let Ok(snapshot) = rx.recv().await {
            for (page, frame, row) in pages.iter() {
                if frame.set_has_data(page.has_data(&snapshot)) {
                    row.set_subtitle(&gettext("Unavailable"));
                    continue;
                }
                page.update(&snapshot);
                if let Some(text) = page.summary(&snapshot) {
                    row.set_subtitle(&text);
                }
            }
        }
    });

    // Closing the window drops the handle: the sampler stops, the channel
    // closes, and the update loop above ends.
    let handle_close = handle.clone();
    window.connect_close_request(move |_| {
        handle_close.borrow_mut().take();
        glib::Propagation::Proceed
    });

    window.present();
}

/// "What's New": the subject, body and hash of the commit this build came
/// from. Release notes accept only a little markup (`<p>`, `<code>`, …), so
/// the commit text is escaped.
fn add_whats_new(about: &adw::AboutDialog) {
    let Some(commit) = config::commit() else {
        return;
    };
    let short = &commit.hash[..commit.hash.len().min(7)];
    let mut notes = format!("<p>{}</p>", glib::markup_escape_text(commit.subject));
    for paragraph in &commit.paragraphs {
        notes.push_str(&format!("<p>{}</p>", glib::markup_escape_text(paragraph)));
    }
    notes.push_str(&format!(
        "<p>{}</p>",
        crate::i18n::fmt(
            // Translators: the git commit this build came from, e.g. "Commit a6f02ff"
            &gettext("Commit {hash}"),
            &[("hash", &format!("<code>{short}</code>"))],
        )
    ));
    about.set_release_notes_version(config::VERSION);
    about.set_release_notes(&notes);
    about.add_link(
        &gettext("Latest Commit"),
        &format!(
            "https://github.com/amirmwhdi/CoreWatch/commit/{}",
            commit.hash
        ),
    );
}
