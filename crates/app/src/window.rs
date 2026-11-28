//! Builds the main window from the module list and feeds it snapshots.
//!
//! The layout lives in `data/ui/stable/window.blp` (the `CwWindow` template).
//! This file never names a resource: it only loops over `modules::all()`.

use crate::modules::{self, ResourcePage};
use crate::settings::StartPage;
use crate::widgets::PageFrame;
use crate::{config, logging};
use adw::prelude::*;
use adw::subclass::prelude::*;
use corewatch_core::{Sampler, Snapshot, SysRoot};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
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
    // One window: shown again when Corewatch is opened while it runs (also
    // when it was hidden to keep watching in the background).
    if let Some(window) = app.windows().first() {
        window.present();
        return;
    }

    let root = SysRoot::from_env();
    logging::log_startup_banner(&root);
    let settings = crate::settings::AppSettings::load();
    settings.apply_theme();
    crate::format::set_units(settings.units);
    crate::widgets::graph::set_history(settings.speed.history_points(settings.history_minutes));
    crate::widgets::graph::set_smooth(settings.smooth_graphs);
    crate::widgets::graph::follow_power_saver();
    crate::drive_smart::start();

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
    // Open on the chosen page, or where the user left off; the first page
    // when that page is not available on this machine.
    let start = match &settings.start_page {
        StartPage::Last => settings.last_page.as_str(),
        StartPage::Page(id) => id.as_str(),
    };
    let start_row = (0..)
        .map_while(|i| sidebar.row_at_index(i))
        .find(|row| row.widget_name() == start)
        .or_else(|| sidebar.row_at_index(0));
    sidebar.select_row(start_row.as_ref());
    let page_list: Rc<Vec<(String, String)>> = Rc::new(
        pages
            .iter()
            .map(|(page, _, _)| (page.id().to_owned(), page.title()))
            .collect(),
    );

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
        #[strong]
        page_list,
        move |_, _| {
            let dialog = crate::preferences::PreferencesDialog::new(handle.clone(), &page_list);
            dialog.present(Some(&window));
        }
    ));
    app.add_action(&pref_action);

    let shortcuts_action = gio::SimpleAction::new("shortcuts", None);
    shortcuts_action.connect_activate(glib::clone!(
        #[weak]
        window,
        move |_, _| crate::compat::show_shortcuts(window.upcast_ref(), crate::shortcuts::groups())
    ));
    app.add_action(&shortcuts_action);

    // Quit ends Corewatch, also when it keeps watching in the background.
    let quit_action = gio::SimpleAction::new("quit", None);
    quit_action.connect_activate(glib::clone!(
        #[weak]
        app,
        #[weak]
        stack,
        move |_, _| {
            save_last_page(&stack);
            app.quit();
        }
    ));
    app.add_action(&quit_action);

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

    // Overview cards and alerts open their page: `app.show-page("cpu")`.
    let show_page = gio::SimpleAction::new("show-page", Some(glib::VariantTy::STRING));
    show_page.connect_activate(glib::clone!(
        #[weak]
        sidebar,
        #[weak]
        window,
        move |_, param| {
            let Some(id) = param.and_then(|p| p.str()) else {
                return;
            };
            // An alert's notification may come while the window is hidden.
            window.present();
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

    // Ctrl+1 … Ctrl+9: the pages in sidebar order.
    let show_page_number = gio::SimpleAction::new("show-page-number", Some(glib::VariantTy::INT32));
    show_page_number.connect_activate(glib::clone!(
        #[weak]
        sidebar,
        move |_, param| {
            // `(2)` in an action name is an int32, so the type must be one.
            let Some(n) = param.and_then(|p| p.get::<i32>()) else {
                return;
            };
            if let Some(row) = sidebar.row_at_index(n - 1) {
                sidebar.select_row(Some(&row));
            }
        }
    ));
    app.add_action(&show_page_number);

    let pages = Rc::new(pages);

    // Ctrl+F: search on the open page, otherwise on the first page that has
    // a search (the process list).
    let find_action = gio::SimpleAction::new("find", None);
    find_action.connect_activate(glib::clone!(
        #[weak]
        stack,
        #[weak]
        sidebar,
        #[weak]
        split,
        #[strong]
        pages,
        move |_, _| {
            let open = stack.visible_child_name();
            let searchable = || pages.iter().filter(|(page, _, _)| page.has_search());
            let target = searchable()
                .find(|(page, _, _)| open.as_deref() == Some(page.id()))
                .or_else(|| searchable().next());
            if let Some((page, _, row)) = target {
                sidebar.select_row(Some(row));
                split.set_show_content(true);
                page.focus_search();
            }
        }
    ));
    app.add_action(&find_action);
    crate::shortcuts::set_accels(app);

    let handle_loop = Rc::downgrade(&handle);
    let app_loop = app.downgrade();
    glib::spawn_future_local(async move {
        while let Ok(snapshot) = rx.recv().await {
            if let Some(app) = app_loop.upgrade() {
                crate::alerts::check(app.upcast_ref(), &snapshot);
            }
            for (page, frame, row) in pages.iter() {
                if frame.set_has_data(page.has_data(&snapshot)) {
                    row.set_subtitle("Unavailable");
                    continue;
                }
                page.update(&snapshot);
                if let Some(text) = page.summary(&snapshot) {
                    row.set_subtitle(&text);
                }
            }
        }
        // The channel closed. When the window closed, the handle is gone
        // and this is the normal end; otherwise the sampler thread died, and
        // every page shows "Data unavailable" instead of frozen numbers.
        if handle_loop
            .upgrade()
            .is_some_and(|handle| handle.borrow().is_some())
        {
            tracing::error!("the sampler thread stopped");
            for (_, frame, row) in pages.iter() {
                while !frame.set_has_data(false) {}
                row.set_subtitle("Unavailable");
            }
        }
    });

    // Closing the window drops the handle: the sampler stops, the channel
    // closes, and the update loop above ends.
    let handle_close = handle.clone();
    let stack_close = stack.downgrade();
    let told_background = Cell::new(false);
    window.connect_close_request(move |window| {
        if let Some(stack) = stack_close.upgrade() {
            save_last_page(&stack);
        }
        // Keep watching: hide the window and keep sampling. Hidden, the pages
        // draw nothing and the process list is not scanned every tick.
        let alerts = crate::settings::AppSettings::load_cached().alerts;
        if alerts.enabled && alerts.background {
            window.set_visible(false);
            if !told_background.replace(true) {
                tell_background(window);
            }
            return glib::Propagation::Stop;
        }
        handle_close.borrow_mut().take();
        glib::Propagation::Proceed
    });

    window.present();
}

/// Remember the open page for "Open On: Last Opened Page".
fn save_last_page(stack: &gtk::Stack) {
    if let Some(id) = stack.visible_child_name() {
        let mut settings = crate::settings::AppSettings::load();
        settings.last_page = id.into();
        settings.save();
    }
}

/// The first time the window hides, say that Corewatch still runs.
fn tell_background(window: &Window) {
    let Some(app) = window.application() else {
        return;
    };
    let notification = gio::Notification::new("Corewatch keeps watching");
    notification.set_body(Some(
        "It runs in the background and sends alerts. Open it again to see everything, or quit it from its menu.",
    ));
    notification.set_default_action("app.show-page::overview");
    notification.add_button("Quit", "app.quit");
    app.send_notification(Some("background"), &notification);
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
        crate::format::fmt(
            // the git commit this build came from, e.g. "Commit a6f02ff"
            "Commit {hash}",
            &[("hash", &format!("<code>{short}</code>"))],
        )
    ));
    about.set_release_notes_version(config::VERSION);
    about.set_release_notes(&notes);
    about.add_link(
        "Latest Commit",
        &format!(
            "https://github.com/amirmwhdi/CoreWatch/commit/{}",
            commit.hash
        ),
    );
}
