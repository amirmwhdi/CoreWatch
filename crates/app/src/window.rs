//! Builds the main window from the module list and feeds it snapshots.
//!
//! This file never names a resource: it only loops over `modules::all()`.

use crate::modules::{self, ResourcePage};
use crate::{config, logging};
use adw::prelude::*;
use corewatch_core::{Sampler, Snapshot, SysRoot};
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

pub fn present(app: &adw::Application) {
    if let Some(window) = app.active_window() {
        window.present();
        return;
    }

    let root = SysRoot::from_env();
    logging::log_startup_banner(&root);
    let mut sampler = Sampler::new(root.clone(), config::SAMPLE_INTERVAL);

    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .build();
    let sidebar = gtk::ListBox::builder()
        .css_classes(["navigation-sidebar"])
        .build();

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

    // Split view: sidebar | content.
    let sidebar_view = adw::ToolbarView::new();
    sidebar_view.add_top_bar(&adw::HeaderBar::new());
    sidebar_view.set_content(Some(&sidebar));

    let content_view = adw::ToolbarView::new();
    content_view.add_top_bar(&adw::HeaderBar::new());
    content_view.set_content(Some(&content_or_empty(&stack, pages.is_empty())));
    let content_page = adw::NavigationPage::new(&content_view, config::APP_NAME);

    let split = adw::NavigationSplitView::builder()
        .sidebar(&adw::NavigationPage::new(&sidebar_view, config::APP_NAME))
        .content(&content_page)
        .build();

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

    let window = adw::ApplicationWindow::builder()
        .application(app)
        .title(config::APP_NAME)
        .default_width(960)
        .default_height(640)
        .width_request(360)
        .height_request(294)
        .content(&split)
        .build();

    // Collapse to one pane on narrow windows.
    match adw::BreakpointCondition::parse("max-width: 600sp") {
        Ok(condition) => {
            let breakpoint = adw::Breakpoint::new(condition);
            breakpoint.add_setter(&split, "collapsed", Some(&true.to_value()));
            window.add_breakpoint(breakpoint);
        }
        Err(error) => tracing::warn!(%error, "breakpoint not set"),
    }

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

/// The page stack, or a status page if every module failed to start.
fn content_or_empty(stack: &gtk::Stack, empty: bool) -> gtk::Widget {
    if !empty {
        return stack.clone().upcast();
    }
    adw::StatusPage::builder()
        .icon_name("dialog-warning-symbolic")
        .title("No data sources available")
        .description(
            "Corewatch could not read /proc. Run corewatch --verbose from a terminal for details.",
        )
        .build()
        .upcast()
}
