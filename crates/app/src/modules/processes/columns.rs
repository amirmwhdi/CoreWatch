//! The table's columns: a label (and, for the name, an icon and the
//! group arrow) bound to one property of the row.

use super::row;
use adw::prelude::*;

/// A text column bound to `property`; sortable by the numeric or string
/// property `sort_by` when given. Only the name column expands; text columns
/// are left-aligned, numbers right-aligned.
pub(super) fn add_column(
    columns: &gtk::ColumnView,
    title: &str,
    property: &'static str,
    sort_by: Option<&'static str>,
    expand: bool,
) -> gtk::ColumnViewColumn {
    let text = matches!(property, "name" | "user");
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(move |_, item| {
        let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
            return;
        };
        let label = gtk::Label::builder()
            .xalign(if text { 0.0 } else { 1.0 })
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        if !text {
            label.add_css_class("numeric");
        }
        // Each item is a tree row; its own item is the process (or app).
        let process = item
            .property_expression("item")
            .chain_property::<gtk::TreeListRow>("item");
        process.chain_property::<row::ProcessRow>(property).bind(
            &label,
            "label",
            gtk::Widget::NONE,
        );
        if property != "name" {
            item.set_child(Some(&label));
            return;
        }
        // The name column also shows the app's icon, and the arrow that
        // opens an app's processes.
        let icon = gtk::Image::builder().pixel_size(24).build();
        process
            .chain_property::<row::ProcessRow>("icon")
            .bind(&icon, "gicon", gtk::Widget::NONE);
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        cell.append(&icon);
        cell.append(&label);
        let expander = gtk::TreeExpander::builder()
            .child(&cell)
            .indent_for_icon(false)
            .build();
        item.property_expression("item")
            .bind(&expander, "list-row", gtk::Widget::NONE);
        item.set_child(Some(&expander));
    });
    let column = gtk::ColumnViewColumn::builder()
        .title(title)
        .factory(&factory)
        .expand(expand)
        .resizable(true)
        .build();
    if let Some(sort_by) = sort_by {
        let expr = gtk::PropertyExpression::new(
            row::ProcessRow::static_type(),
            gtk::Expression::NONE,
            sort_by,
        );
        let sorter: gtk::Sorter = if sort_by == "user" {
            gtk::StringSorter::new(Some(expr)).upcast()
        } else {
            gtk::NumericSorter::new(Some(expr)).upcast()
        };
        column.set_sorter(Some(&sorter));
    }
    columns.append_column(&column);
    column
}
