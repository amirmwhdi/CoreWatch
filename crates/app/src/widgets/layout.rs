//! Layout helpers shared by every module page, so pages look the same.

use adw::prelude::*;

/// Vertical content box with the standard page margins.
pub fn page_box() -> gtk::Box {
    let content = gtk::Box::new(gtk::Orientation::Vertical, 18);
    content.set_margin_top(24);
    content.set_margin_bottom(24);
    content.set_margin_start(12);
    content.set_margin_end(12);
    content
}

/// Clamp the content width and make it scrollable.
pub fn scrolled(content: &gtk::Box) -> gtk::Widget {
    let clamp = adw::Clamp::builder()
        .maximum_size(1000)
        .child(content)
        .build();
    gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .child(&clamp)
        .build()
        .upcast()
}
