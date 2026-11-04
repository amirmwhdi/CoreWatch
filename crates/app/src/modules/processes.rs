//! Processes page: every process in a sortable, searchable table, with a
//! button to end the selected one.
//!
//! Rows are GObjects kept per pid and updated in place, so a tick changes a
//! few properties instead of rebuilding the table, and only the rows on
//! screen redraw.

use super::{Module, ResourcePage};
use crate::app_icons::AppIcons;
use crate::format;
use adw::prelude::*;
use adw::subclass::prelude::ObjectSubclassIsExt;
use corewatch_core::process::{
    current_uid, send_signal, ProcessCollector, ProcessInfo, ProcessSignal, ProcessSource,
};
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

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
        filter.append(search);
        filter.append(owner);

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
        group_toggle.connect_toggled(glib::clone!(
            #[weak]
            sorted,
            #[strong]
            store,
            #[strong]
            groups,
            move |toggle| {
                let grouped = toggle.is_active();
                sorted.set_model(Some(&tree_of(if grouped { &groups } else { &store })));
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
            generation: Cell::new(0),
            units: Cell::new(format::units_generation()),
            count: Cell::new(0),
            root: view.upcast(),
        }
    }

    /// `nvml`: GPU usage by pid that NVIDIA's driver reports, merged with the
    /// usage the processes' own GPU files give.
    fn sync(&self, processes: &[ProcessInfo], nvml: &HashMap<u32, f32>) {
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
            sorter.changed(gtk::SorterChange::Different);
        }
    }
}

/// The group of kernel threads; no app ID or unit name looks like this.
const KERNEL: &str = "\0kernel";

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

/// A text column bound to `property`; sortable by the numeric or string
/// property `sort_by` when given. Only the name column expands; text columns
/// are left-aligned, numbers right-aligned.
fn add_column(
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

/// Ask before ending a process, then send the signal off the main thread.
pub(crate) fn confirm_end(
    parent: &gtk::Widget,
    toasts: &adw::ToastOverlay,
    process: &row::ProcessRow,
) {
    // A group ends all its processes.
    let targets: Vec<(u32, u64)> = match process.children() {
        Some(children) => children
            .iter::<row::ProcessRow>()
            .flatten()
            .map(|r| r.key())
            .collect(),
        None => vec![process.key()],
    };
    let heading = if process.is_group() {
        format::fmt(
            format::plural(
                targets.len(),
                "End the process of “{name}”?",
                "End all {n} processes of “{name}”?",
            ),
            &[("name", &process.group_name()), ("n", &targets.len())],
        )
    } else {
        format::fmt("End “{name}”?", &[("name", &process.name())])
    };
    let dialog = adw::AlertDialog::new(
        Some(&heading),
        Some("Unsaved work in this program may be lost. Force Stop ends it at once, without letting it clean up."),
    );
    dialog.add_responses(&[
        ("cancel", "_Cancel"),
        ("kill", "_Force Stop"),
        ("end", "_End Process"),
    ]);
    dialog.set_response_appearance("kill", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("end", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("end"));
    dialog.set_close_response("cancel");

    let toasts = toasts.clone();
    let source = ProcessSource::detect(&SysRoot::from_env());
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |response| {
        let signal = match response.as_str() {
            "end" => ProcessSignal::End,
            "kill" => ProcessSignal::ForceStop,
            _ => return,
        };
        glib::spawn_future_local(async move {
            let count = targets.len();
            let results = gio::spawn_blocking(move || {
                targets
                    .into_iter()
                    .map(|(pid, start_ticks)| {
                        let result = send_signal(source, pid, start_ticks, signal);
                        if let Err(error) = &result {
                            if error.kind() != io::ErrorKind::NotFound {
                                tracing::warn!(pid, %error, "cannot end process");
                            }
                        }
                        result
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default();
            // Gone already counts as ended.
            let failed: Vec<&io::Error> = results
                .iter()
                .filter_map(|r| r.as_ref().err())
                .filter(|e| e.kind() != io::ErrorKind::NotFound)
                .collect();
            let message = if results.is_empty() {
                String::from("Could not end the process")
            } else if failed.is_empty() {
                if count == 1 && results.iter().all(Result::is_err) {
                    String::from("The process has already ended")
                } else {
                    return;
                }
            } else if failed
                .iter()
                .all(|e| e.kind() == io::ErrorKind::PermissionDenied)
            {
                String::from("You can only end your own processes")
            } else if count == 1 {
                String::from("Could not end the process")
            } else {
                format::fmt(
                    "Could not end {failed} of {count} processes",
                    &[("failed", &failed.len()), ("count", &count)],
                )
            };
            toasts.add_toast(adw::Toast::new(&message));
        });
    });
}

/// `CwProcessRow`: one table row. Numbers are kept for sorting, text for
/// display; both are set only when they change.
pub(crate) mod row {
    use super::*;
    use glib::subclass::prelude::*;

    mod imp {
        use super::*;

        #[derive(Default, glib::Properties)]
        #[properties(wrapper_type = super::ProcessRow)]
        pub struct ProcessRow {
            #[property(get)]
            pub pid: Cell<u32>,
            pub start_ticks: Cell<u64>,
            /// The app or service this process belongs to (see
            /// `cgroup::group`); for a group row, the group itself.
            pub group: RefCell<Option<Arc<str>>>,
            /// A group row: one app's processes, in `children`.
            pub children: RefCell<Option<gio::ListStore>>,
            /// A group row's name, without the count `name` adds.
            pub group_name: RefCell<String>,
            #[property(get)]
            pub pid_text: RefCell<String>,
            /// `u32::MAX` when unknown.
            #[property(get)]
            pub uid: Cell<u32>,
            #[property(get)]
            pub name: RefCell<String>,
            #[property(get)]
            pub icon: RefCell<Option<gio::Icon>>,
            #[property(get)]
            pub user: RefCell<String>,
            #[property(get)]
            pub cpu: Cell<f32>,
            #[property(get)]
            pub cpu_text: RefCell<String>,
            #[property(get)]
            pub memory: Cell<u64>,
            #[property(get)]
            pub memory_text: RefCell<String>,
            /// Bytes per second; 0 when unknown, so unknown sorts with idle.
            #[property(get)]
            pub download: Cell<u64>,
            #[property(get)]
            pub download_text: RefCell<String>,
            #[property(get)]
            pub upload: Cell<u64>,
            #[property(get)]
            pub upload_text: RefCell<String>,
            /// 0.0..=1.0; 0 when unknown, so unknown sorts with idle.
            #[property(get)]
            pub gpu: Cell<f32>,
            #[property(get)]
            pub gpu_text: RefCell<String>,
        }

        #[glib::object_subclass]
        impl ObjectSubclass for ProcessRow {
            const NAME: &'static str = "CwProcessRow";
            type Type = super::ProcessRow;
        }

        #[glib::derived_properties]
        impl ObjectImpl for ProcessRow {}
    }

    glib::wrapper! {
        pub struct ProcessRow(ObjectSubclass<imp::ProcessRow>);
    }

    impl ProcessRow {
        pub fn new(info: &ProcessInfo, gpu: Option<f32>, icons: &AppIcons) -> Self {
            let row: Self = glib::Object::new();
            let imp = row.imp();
            imp.pid.set(info.pid);
            imp.start_ticks.set(info.start_ticks);
            *imp.group.borrow_mut() = info.group.clone();
            *imp.pid_text.borrow_mut() = info.pid.to_string();
            imp.uid.set(info.uid.unwrap_or(u32::MAX));
            *imp.user.borrow_mut() = info.user.to_string();
            *imp.name.borrow_mut() = info.name.to_string();
            *imp.icon.borrow_mut() = Some(icons.lookup(&info.name));
            imp.cpu.set(info.cpu);
            *imp.cpu_text.borrow_mut() = cpu_text(info.cpu);
            imp.memory.set(info.memory);
            *imp.memory_text.borrow_mut() = format::size(info.memory);
            imp.download.set(info.download.unwrap_or(0));
            *imp.download_text.borrow_mut() = speed_text(info.download);
            imp.upload.set(info.upload.unwrap_or(0));
            *imp.upload_text.borrow_mut() = speed_text(info.upload);
            imp.gpu.set(gpu.unwrap_or(0.0));
            *imp.gpu_text.borrow_mut() = gpu_text(gpu);
            row
        }

        /// An empty group row for `group`; [`ProcessRow::set_group`] fills it.
        pub fn new_group(group: Option<Arc<str>>) -> Self {
            let row: Self = glib::Object::new();
            let imp = row.imp();
            *imp.group.borrow_mut() = group;
            *imp.children.borrow_mut() = Some(gio::ListStore::new::<ProcessRow>());
            row
        }

        pub fn group(&self) -> Option<Arc<str>> {
            self.imp().group.borrow().clone()
        }

        /// The processes of a group row; `None` for a process row.
        pub fn children(&self) -> Option<gio::ListStore> {
            self.imp().children.borrow().clone()
        }

        pub fn group_name(&self) -> String {
            self.imp().group_name.borrow().clone()
        }

        pub fn is_group(&self) -> bool {
            self.imp().children.borrow().is_some()
        }

        /// Whether End Process applies: a process, or an app's group. Not
        /// the Kernel group (kernel threads cannot be ended) nor Other
        /// Processes, which holds processes that have nothing to do with
        /// each other.
        pub fn can_end(&self) -> bool {
            !self.is_group() || !matches!(self.group().as_deref(), None | Some(KERNEL))
        }

        /// A group row's name and totals, from its processes.
        pub fn set_group(&self, name: &str, icon: gio::Icon, members: &[ProcessRow]) {
            let imp = self.imp();
            if *imp.group_name.borrow() != name {
                *imp.group_name.borrow_mut() = name.to_owned();
            }
            let label = format!("{name} ({})", members.len());
            if *imp.name.borrow() != label {
                *imp.name.borrow_mut() = label;
                self.notify_name();
            }
            if imp.icon.borrow().as_ref() != Some(&icon) {
                *imp.icon.borrow_mut() = Some(icon);
                self.notify_icon();
            }
            // One user for all, or none shown.
            let first = members.first().map(|m| (m.uid(), m.user()));
            let same = members
                .iter()
                .all(|m| Some(m.uid()) == first.as_ref().map(|f| f.0));
            let (uid, user) = match first {
                Some((uid, user)) if same => (uid, user),
                _ => (u32::MAX, String::new()),
            };
            imp.uid.set(uid);
            set_text(&imp.user, user, || self.notify_user());

            let cpu: f32 = members.iter().map(ProcessRow::cpu).sum();
            if imp.cpu.replace(cpu) != cpu {
                self.notify_cpu();
            }
            set_text(&imp.cpu_text, cpu_text(cpu), || self.notify_cpu_text());
            let memory: u64 = members.iter().map(ProcessRow::memory).sum();
            if imp.memory.replace(memory) != memory {
                self.notify_memory();
            }
            set_text(&imp.memory_text, format::size(memory), || {
                self.notify_memory_text()
            });
            let gpu = members.iter().map(ProcessRow::gpu).sum::<f32>().min(1.0);
            if imp.gpu.replace(gpu) != gpu {
                self.notify_gpu();
            }
            let known = |text: String| {
                members
                    .iter()
                    .any(|m| m.imp().gpu_text.borrow().as_str() != "—")
                    .then_some(text)
            };
            set_text(
                &imp.gpu_text,
                known(gpu_text(Some(gpu))).unwrap_or_else(|| "—".to_owned()),
                || self.notify_gpu_text(),
            );
            let download: u64 = members.iter().map(ProcessRow::download).sum();
            if imp.download.replace(download) != download {
                self.notify_download();
            }
            set_text(&imp.download_text, speed_text(Some(download)), || {
                self.notify_download_text()
            });
            let upload: u64 = members.iter().map(ProcessRow::upload).sum();
            if imp.upload.replace(upload) != upload {
                self.notify_upload();
            }
            set_text(&imp.upload_text, speed_text(Some(upload)), || {
                self.notify_upload_text()
            });
        }

        pub fn set_icon(&self, icon: gio::Icon) {
            *self.imp().icon.borrow_mut() = Some(icon);
            self.notify_icon();
        }

        /// Pid and start time: which process this row is.
        pub fn key(&self) -> (u32, u64) {
            (self.imp().pid.get(), self.imp().start_ticks.get())
        }

        /// `relabel` remakes every text, after the units changed.
        pub fn update(
            &self,
            info: &ProcessInfo,
            gpu: Option<f32>,
            icons: &AppIcons,
            relabel: bool,
        ) {
            let imp = self.imp();
            if *imp.name.borrow() != *info.name {
                *imp.name.borrow_mut() = info.name.to_string();
                self.notify_name();
                *imp.icon.borrow_mut() = Some(icons.lookup(&info.name));
                self.notify_icon();
            }
            // A user's name can arrive after the row was made (a slow user
            // database): the uid is shown until then.
            if *imp.user.borrow() != *info.user {
                *imp.user.borrow_mut() = info.user.to_string();
                self.notify_user();
            }
            if imp.cpu.replace(info.cpu) != info.cpu {
                self.notify_cpu();
                set_text(&imp.cpu_text, cpu_text(info.cpu), || self.notify_cpu_text());
            }
            if imp.memory.replace(info.memory) != info.memory || relabel {
                self.notify_memory();
                set_text(&imp.memory_text, format::size(info.memory), || {
                    self.notify_memory_text()
                });
            }
            let download = info.download.unwrap_or(0);
            if imp.download.replace(download) != download {
                self.notify_download();
            }
            set_text(&imp.download_text, speed_text(info.download), || {
                self.notify_download_text()
            });
            let upload = info.upload.unwrap_or(0);
            if imp.upload.replace(upload) != upload {
                self.notify_upload();
            }
            set_text(&imp.upload_text, speed_text(info.upload), || {
                self.notify_upload_text()
            });
            let share = gpu.unwrap_or(0.0);
            if imp.gpu.replace(share) != share {
                self.notify_gpu();
            }
            set_text(&imp.gpu_text, gpu_text(gpu), || self.notify_gpu_text());
        }
    }

    fn set_text(cell: &RefCell<String>, text: String, notify: impl FnOnce()) {
        if *cell.borrow() != text {
            *cell.borrow_mut() = text;
            notify();
        }
    }

    /// Empty for an idle process, a dash when the speed is unknown (another
    /// user's process).
    fn speed_text(rate: Option<u64>) -> String {
        match rate {
            Some(0) => String::new(),
            Some(rate) => format::net_speed(rate),
            None => "—".to_owned(),
        }
    }

    /// Empty for a process that does not use the GPU, a dash when unknown
    /// (another user's process).
    fn gpu_text(share: Option<f32>) -> String {
        match share {
            Some(s) if s < 0.0005 => String::new(),
            Some(s) => cpu_text(s),
            None => "—".to_owned(),
        }
    }

    /// One decimal below 10%, so light processes are not all "0%".
    fn cpu_text(share: f32) -> String {
        let value = share * 100.0;
        let value = if value < 10.0 {
            format!("{value:.1}")
        } else {
            format!("{value:.0}")
        };
        // a percentage, e.g. "37%"
        format::fmt("{value}%", &[("value", &value)])
    }
}

/// The `CwProcessesPage` template from `data/ui/stable/processes-page.blp`.
mod view {
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
