//! Processes page: every process in a sortable, searchable table, with a
//! button to end the selected one.
//!
//! - `row`: `CwProcessRow`, the GObject behind each line of the table;
//! - `columns`: the table's columns and how each cell is drawn;
//! - `end`: the End Process dialog and the signal it sends;
//! - `view`: the Blueprint template of the page.
//!
//! Rows are GObjects kept per pid and updated in place, so a tick changes a
//! few properties instead of rebuilding the table, and only the rows on
//! screen redraw.

use super::{Module, ResourcePage};
use crate::app_icons::AppIcons;
use crate::format;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::process::{current_uid, ProcessCollector, ProcessInfo, ProcessSource};
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

mod columns;
mod end;
pub(crate) mod row;
mod view;

use columns::add_column;
pub(crate) use end::confirm_end;

/// Set while the page is on screen; the collector scans every tick only then.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn collector(root: &SysRoot) -> Result<Box<dyn Collector>, CollectError> {
    Ok(Box::new(ProcessCollector::new(
        root,
        ProcessSource::detect(root),
        &ACTIVE,
    )?))
}

fn page() -> Box<dyn ResourcePage> {
    Box::new(ProcessesPage::new())
}

pub const MODULE: Module = Module {
    collector: Some(collector),
    page,
};

struct ProcessesPage {
    root: gtk::Widget,
    search: gtk::SearchEntry,
    /// Every process, for the plain list.
    store: gio::ListStore,
    /// One row per app or service, its processes as children.
    groups: gio::ListStore,
    group_rows: RefCell<HashMap<Option<Arc<str>>, row::ProcessRow>>,
    /// By pid and start time: a pid reused by a new process gets a new row,
    /// not the old process's user and icon.
    rows: Rc<RefCell<HashMap<(u32, u64), row::ProcessRow>>>,
    /// App icons, read on a worker thread the first time the list is shown;
    /// the fallback icon until then.
    icons: Rc<RefCell<AppIcons>>,
    columns: gtk::ColumnView,
    /// Set while the page sorts again after an update, so the sorter's
    /// `changed` is not taken for a click on a column header.
    resorting: Rc<Cell<bool>>,
    generation: Cell<u64>,
    /// The units the row texts were made with.
    units: Cell<u32>,
    count: Cell<usize>,
}

impl ProcessesPage {
    fn new() -> Self {
        let view = view::ProcessesView::new();
        let ui = view.imp();
        let columns = ui.columns.get();
        let store = gio::ListStore::new::<row::ProcessRow>();

        // Search by name (any part, any case) or by process ID (its start),
        // optionally only the user's own processes.
        let search_entry = ui.search.get();
        let search = gtk::CustomFilter::new(glib::clone!(
            #[weak]
            search_entry,
            #[upgrade_or]
            true,
            move |item| {
                let query = search_entry.text().trim().to_owned();
                let matches =
                    |r: &row::ProcessRow| matches_search(&r.name(), &r.pid().to_string(), &query);
                // A group shows when its name or one of its processes matches.
                query.is_empty()
                    || item
                        .downcast_ref::<row::ProcessRow>()
                        .is_some_and(|r| matches(r) || any_child(r, matches))
            }
        ));
        search_entry.connect_search_changed(glib::clone!(
            #[weak]
            search,
            move |_| search.changed(gtk::FilterChange::Different)
        ));
        // Typing anywhere on the page starts a search.
        search_entry.set_key_capture_widget(Some(&view));
        let name_expr = gtk::PropertyExpression::new(
            row::ProcessRow::static_type(),
            gtk::Expression::NONE,
            "name",
        );
        let my_uid = current_uid();
        let mine = ui.mine.get();
        let owner = gtk::CustomFilter::new(glib::clone!(
            #[weak]
            mine,
            #[upgrade_or]
            true,
            move |item| {
                let own = |r: &row::ProcessRow| r.uid() == my_uid;
                !mine.is_active()
                    || item
                        .downcast_ref::<row::ProcessRow>()
                        .is_some_and(|r| own(r) || any_child(r, own))
            }
        ));
        mine.connect_toggled(glib::clone!(
            #[weak]
            owner,
            move |_| owner.changed(gtk::FilterChange::Different)
        ));
        let filter = gtk::EveryFilter::new();
        filter.append(search.clone());
        filter.append(owner.clone());

        // Rows are a tree: processes, or apps that open to their processes.
        // Either list is filtered at the top level and sorted within each
        // level, so an app's processes stay under it.
        let groups = gio::ListStore::new::<row::ProcessRow>();
        let tree_of = {
            let filter = filter.clone();
            move |list: &gio::ListStore| {
                let filtered = gtk::FilterListModel::new(Some(list.clone()), Some(filter.clone()));
                // A group's processes are filtered too, so a search shows only
                // the processes that match under their app.
                let child_filter = filter.clone();
                gtk::TreeListModel::new(filtered, false, false, move |item| {
                    let children = item.downcast_ref::<row::ProcessRow>()?.children()?;
                    Some(
                        gtk::FilterListModel::new(Some(children), Some(child_filter.clone()))
                            .upcast::<gio::ListModel>(),
                    )
                })
            }
        };
        let grouped = crate::settings::AppSettings::load().group_processes;
        let group_toggle = ui.group.get();
        group_toggle.set_active(grouped);
        let tree = tree_of(if grouped { &groups } else { &store });
        let sorted = gtk::SortListModel::new(
            Some(tree),
            Some(gtk::TreeListRowSorter::new(columns.sorter())),
        );
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        columns.set_model(Some(&selection));
        let columns_weak = columns.downgrade();
        group_toggle.connect_toggled(glib::clone!(
            #[weak]
            sorted,
            #[strong(rename_to = columns)]
            columns_weak,
            #[strong]
            store,
            #[strong]
            groups,
            move |toggle| {
                let grouped = toggle.is_active();
                sorted.set_model(Some(&tree_of(if grouped { &groups } else { &store })));
                if let Some(columns) = columns.upgrade() {
                    scroll_to_top_later(&columns);
                }
                let mut settings = crate::settings::AppSettings::load();
                settings.group_processes = grouped;
                settings.save();
            }
        ));

        let name = add_column(&columns, "Name", "name", None, true);
        name.set_sorter(Some(&gtk::StringSorter::new(Some(name_expr))));
        add_column(&columns, "User", "user", Some("user"), false);
        let cpu = add_column(&columns, "CPU", "cpu-text", Some("cpu"), false);
        add_column(&columns, "Memory", "memory-text", Some("memory"), false);
        add_column(&columns, "GPU", "gpu-text", Some("gpu"), false);
        add_column(
            &columns,
            "Download",
            "download-text",
            Some("download"),
            false,
        );
        add_column(&columns, "Upload", "upload-text", Some("upload"), false);
        add_column(&columns, "ID", "pid-text", Some("pid"), false);
        columns.sort_by_column(Some(&cpu), gtk::SortType::Descending);

        // A new order or a new set of rows starts at the top. Without this
        // GTK keeps the row that was at the top in view, wherever the new
        // order puts it, and the best matches are scrolled away.
        let resorting = Rc::new(Cell::new(false));
        if let Some(sorter) = columns.sorter() {
            sorter.connect_changed(glib::clone!(
                #[weak]
                columns,
                #[strong]
                resorting,
                move |_, _| {
                    // The page's own re-sort after an update is not a new order.
                    if !resorting.get() {
                        scroll_to_top_later(&columns);
                    }
                }
            ));
        }
        for filter in [search.upcast_ref::<gtk::Filter>(), owner.upcast_ref()] {
            filter.connect_changed(glib::clone!(
                #[weak]
                columns,
                move |_, _| scroll_to_top_later(&columns)
            ));
        }
        // Enter or a double click: a group opens or closes, a process shows
        // its details.
        columns.connect_activate(glib::clone!(
            #[weak]
            selection,
            move |columns, position| {
                let Some(tree_row) = selection.item(position).and_downcast::<gtk::TreeListRow>()
                else {
                    return;
                };
                match tree_row.item().and_downcast::<row::ProcessRow>() {
                    Some(process) if !process.is_group() => {
                        super::process_details::show(columns.upcast_ref(), &process);
                    }
                    _ => tree_row.set_expanded(!tree_row.is_expanded()),
                }
            }
        ));
        let details_button = ui.details_button.get();
        selection
            .bind_property("selected-item", &details_button, "sensitive")
            .transform_to(|_, item: Option<glib::Object>| {
                let process = item
                    .and_downcast::<gtk::TreeListRow>()
                    .and_then(|r| r.item())
                    .and_downcast::<row::ProcessRow>();
                Some(process.is_some_and(|p| !p.is_group()))
            })
            .sync_create()
            .build();
        details_button.connect_clicked(glib::clone!(
            #[weak]
            selection,
            move |button| {
                if let Some(process) = selected(&selection).filter(|p| !p.is_group()) {
                    super::process_details::show(button.upcast_ref(), &process);
                }
            }
        ));

        // The table only needs fresh data while it is on screen.
        view.connect_map(|_| ACTIVE.store(true, Ordering::Relaxed));
        let rows: Rc<RefCell<HashMap<(u32, u64), row::ProcessRow>>> = Rc::default();
        let icons = Rc::new(RefCell::new(AppIcons::empty()));
        let icons_requested = Cell::new(false);
        view.connect_map(glib::clone!(
            #[strong]
            rows,
            #[strong]
            icons,
            move |_| {
                if icons_requested.replace(true) {
                    return;
                }
                let (rows, icons) = (rows.clone(), icons.clone());
                glib::spawn_future_local(async move {
                    let Ok(table) = gio::spawn_blocking(AppIcons::read).await else {
                        return;
                    };
                    *icons.borrow_mut() = AppIcons::from_table(table);
                    let icons = icons.borrow();
                    for row in rows.borrow().values() {
                        row.set_icon(icons.lookup(&row.name()));
                    }
                });
            }
        ));
        view.connect_unmap(|_| ACTIVE.store(false, Ordering::Relaxed));

        let end_button = ui.end_button.get();
        selection
            .bind_property("selected-item", &end_button, "sensitive")
            .transform_to(|_, item: Option<glib::Object>| {
                let row = item
                    .and_downcast::<gtk::TreeListRow>()
                    .and_then(|r| r.item())
                    .and_downcast::<row::ProcessRow>();
                Some(row.is_some_and(|r| r.can_end()))
            })
            .sync_create()
            .build();
        let toasts = ui.toasts.get();
        end_button.connect_clicked(glib::clone!(
            #[weak]
            selection,
            #[weak]
            toasts,
            move |button| {
                if let Some(process) = selected(&selection).filter(|p| p.can_end()) {
                    confirm_end(button.upcast_ref(), &toasts, &process);
                }
            }
        ));

        // Delete on the list: the same as the End Process button.
        let delete = gtk::ShortcutController::new();
        delete.add_shortcut(gtk::Shortcut::new(
            gtk::ShortcutTrigger::parse_string("Delete"),
            Some(gtk::CallbackAction::new(glib::clone!(
                #[weak]
                end_button,
                #[upgrade_or]
                glib::Propagation::Proceed,
                move |_, _| {
                    if !end_button.is_sensitive() {
                        return glib::Propagation::Proceed;
                    }
                    end_button.emit_clicked();
                    glib::Propagation::Stop
                }
            ))),
        ));
        columns.add_controller(delete);

        Self {
            search: search_entry,
            store,
            groups,
            group_rows: RefCell::default(),
            rows,
            icons,
            columns,
            resorting,
            generation: Cell::new(0),
            units: Cell::new(format::units_generation()),
            count: Cell::new(0),
            root: view.upcast(),
        }
    }

    /// `nvml`: GPU usage by pid that NVIDIA's driver reports, merged with the
    /// usage the processes' own GPU files give.
    fn sync(&self, processes: &[ProcessInfo], nvml: &HashMap<u32, f32>) {
        // At the top, stay at the top: the rows move every update, and GTK
        // would otherwise follow the row that was first to its new place.
        let at_top = self.columns.vadjustment().is_none_or(|a| a.value() < 1.0);
        let icons = self.icons.borrow();
        let units = format::units_generation();
        let relabel = self.units.replace(units) != units;
        let mut rows = self.rows.borrow_mut();
        let mut added = Vec::new();
        let mut alive = HashSet::with_capacity(processes.len());
        for info in processes {
            let key = (info.pid, info.start_ticks);
            alive.insert(key);
            match rows.get(&key) {
                Some(row) => row.update(info, gpu_of(info, nvml), &icons, relabel),
                None => {
                    let row = row::ProcessRow::new(info, gpu_of(info, nvml), &icons);
                    added.push(row.clone());
                    rows.insert(key, row);
                }
            }
        }
        if rows.len() > alive.len() {
            rows.retain(|key, _| alive.contains(key));
            self.store.retain(|item| {
                item.downcast_ref::<row::ProcessRow>()
                    .is_some_and(|r| alive.contains(&r.key()))
            });
        }
        if !added.is_empty() {
            self.store.extend_from_slice(&added);
        }
        self.sync_groups(processes, &rows, &icons);
        // Values changed in place: sort again.
        if let Some(sorter) = self.columns.sorter() {
            self.resorting.set(true);
            sorter.changed(gtk::SorterChange::Different);
            self.resorting.set(false);
        }
        if at_top {
            scroll_to_top(&self.columns);
        }
    }
}

/// Show the first row at the top, without selecting or focusing it.
fn scroll_to_top(columns: &gtk::ColumnView) {
    if columns.model().is_some_and(|m| m.n_items() > 0) {
        columns.scroll_to(0, None, gtk::ListScrollFlags::NONE, None);
    }
}

/// [`scroll_to_top`] once GTK has taken in a new order or a new set of
/// rows: from inside the sorter's or filter's `changed`, the list view has
/// not moved its rows yet and would put the old top row back in view.
fn scroll_to_top_later(columns: &gtk::ColumnView) {
    let columns = columns.downgrade();
    glib::idle_add_local_once(move || {
        if let Some(columns) = columns.upgrade() {
            scroll_to_top(&columns);
        }
    });
}

/// The group of kernel threads; no app ID or unit name looks like this.
pub(crate) const KERNEL: &str = "\0kernel";

impl ProcessesPage {
    /// One group row per app or service, holding the rows of its processes.
    fn sync_groups(
        &self,
        processes: &[ProcessInfo],
        rows: &HashMap<(u32, u64), row::ProcessRow>,
        icons: &AppIcons,
    ) {
        let mut members: HashMap<Option<Arc<str>>, Vec<row::ProcessRow>> = HashMap::new();
        for info in processes {
            if let Some(row) = rows.get(&(info.pid, info.start_ticks)) {
                let key = match (&info.group, info.kernel_thread) {
                    (Some(group), _) => Some(group.clone()),
                    (None, true) => Some(Arc::from(KERNEL)),
                    (None, false) => None,
                };
                members.entry(key).or_default().push(row.clone());
            }
        }
        let mut groups = self.group_rows.borrow_mut();
        // Groups whose processes all ended.
        let before = groups.len();
        groups.retain(|key, _| members.contains_key(key));
        if groups.len() != before {
            self.groups.retain(|item| {
                item.downcast_ref::<row::ProcessRow>()
                    .is_some_and(|r| groups.contains_key(&r.group()))
            });
        }
        let mut added = Vec::new();
        for (key, list) in &members {
            let group = groups.entry(key.clone()).or_insert_with(|| {
                let row = row::ProcessRow::new_group(key.clone());
                added.push(row.clone());
                row
            });
            let (name, icon) = match key.as_deref() {
                Some(KERNEL) => ("Kernel".to_owned(), icons.fallback()),
                None => ("Other Processes".to_owned(), icons.fallback()),
                Some(id) => match icons.app(id) {
                    Some((name, icon)) => (name.to_owned(), icon),
                    None => (id.to_owned(), icons.lookup(id)),
                },
            };
            group.set_group(&name, icon, list);
            if let Some(children) = group.children() {
                sync_children(&children, list);
            }
        }
        if !added.is_empty() {
            self.groups.extend_from_slice(&added);
        }
    }
}

/// Make `children` hold exactly `list`, keeping the rows already there (an
/// open group stays open, a selected process stays selected).
fn sync_children(children: &gio::ListStore, list: &[row::ProcessRow]) {
    let wanted: HashSet<(u32, u64)> = list.iter().map(row::ProcessRow::key).collect();
    let have: HashSet<(u32, u64)> = children
        .iter::<row::ProcessRow>()
        .flatten()
        .map(|r| r.key())
        .collect();
    if have.len() > wanted.len() || have.iter().any(|k| !wanted.contains(k)) {
        children.retain(|item| {
            item.downcast_ref::<row::ProcessRow>()
                .is_some_and(|r| wanted.contains(&r.key()))
        });
    }
    let new: Vec<row::ProcessRow> = list
        .iter()
        .filter(|r| !have.contains(&r.key()))
        .cloned()
        .collect();
    if !new.is_empty() {
        children.extend_from_slice(&new);
    }
}

impl ResourcePage for ProcessesPage {
    fn id(&self) -> &'static str {
        "processes"
    }

    fn title(&self) -> String {
        String::from("Processes")
    }

    fn icon_name(&self) -> &'static str {
        "view-list-symbolic"
    }

    fn widget(&self) -> gtk::Widget {
        self.root.clone()
    }

    fn update(&self, snapshot: &Snapshot) {
        let Some(sample) = &snapshot.processes else {
            return;
        };
        self.count.set(sample.processes.len());
        // Hidden page or no new scan: nothing to do.
        if sample.generation == self.generation.get() || !self.root.is_mapped() {
            return;
        }
        self.generation.set(sample.generation);
        // The one other field this page reads: NVIDIA cards report usage per
        // process to the GPU collector (NVML), not in the processes' files.
        let nvml: HashMap<u32, f32> = snapshot
            .gpu
            .as_ref()
            .map(|g| g.process_usage.iter().copied().collect())
            .unwrap_or_default();
        self.sync(&sample.processes, &nvml);
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.processes.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        let n = snapshot.processes.as_ref()?.processes.len();
        Some(format::fmt(
            format::plural(n, "{n} process", "{n} processes"),
            &[("n", &n)],
        ))
    }

    fn has_search(&self) -> bool {
        true
    }

    fn focus_search(&self) {
        self.search.grab_focus();
        self.search.select_region(0, -1);
    }
}

/// A process matches when its name contains `query` (ignoring case), or
/// when `query` is a number its process ID starts with.
fn matches_search(name: &str, pid: &str, query: &str) -> bool {
    if query.bytes().all(|b| b.is_ascii_digit()) && pid.starts_with(query) {
        return true;
    }
    name.to_lowercase().contains(&query.to_lowercase())
}

/// The process (or app) of the selected row.
fn selected(selection: &gtk::SingleSelection) -> Option<row::ProcessRow> {
    selection
        .selected_item()
        .and_downcast::<gtk::TreeListRow>()?
        .item()
        .and_downcast()
}

/// Whether one of a group row's processes is `f`; false for a process row.
fn any_child(row: &row::ProcessRow, f: impl Fn(&row::ProcessRow) -> bool) -> bool {
    row.children().is_some_and(|children| {
        children
            .iter::<row::ProcessRow>()
            .flatten()
            .any(|child| f(&child))
    })
}

/// A process's GPU usage: the larger of its DRM files' and NVML's.
fn gpu_of(info: &ProcessInfo, nvml: &HashMap<u32, f32>) -> Option<f32> {
    match (info.gpu, nvml.get(&info.pid).copied()) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests {
    use super::matches_search;

    #[test]
    fn search_by_name_or_process_id() {
        assert!(matches_search("Firefox", "4242", "fire"));
        assert!(matches_search("Firefox", "4242", "FOX"));
        assert!(matches_search("Firefox", "4242", "42"));
        assert!(!matches_search("Firefox", "4242", "24"));
        assert!(!matches_search("bash", "1", "fire"));
        // Digits in a name still match the name.
        assert!(matches_search("python3", "900", "3"));
    }
}
