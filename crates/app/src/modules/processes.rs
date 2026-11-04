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
    current_uid, send_signal, ProcessCollector, ProcessInfo, ProcessSource,
};
use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::io;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

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
    store: gio::ListStore,
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
                query.is_empty()
                    || item
                        .downcast_ref::<row::ProcessRow>()
                        .is_some_and(|r| matches_search(&r.name(), &r.pid().to_string(), &query))
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
                !mine.is_active()
                    || item
                        .downcast_ref::<row::ProcessRow>()
                        .is_some_and(|r| r.uid() == my_uid)
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

        let filtered = gtk::FilterListModel::new(Some(store.clone()), Some(filter));
        let sorted = gtk::SortListModel::new(Some(filtered), columns.sorter());
        let selection = gtk::SingleSelection::builder()
            .model(&sorted)
            .autoselect(false)
            .can_unselect(true)
            .build();
        columns.set_model(Some(&selection));

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
            .transform_to(|_, item: Option<glib::Object>| Some(item.is_some()))
            .sync_create()
            .build();
        let toasts = ui.toasts.get();
        end_button.connect_clicked(glib::clone!(
            #[weak]
            selection,
            #[weak]
            toasts,
            move |button| {
                if let Some(process) = selection.selected_item().and_downcast::<row::ProcessRow>() {
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
        // Values changed in place: sort again.
        if let Some(sorter) = self.columns.sorter() {
            sorter.changed(gtk::SorterChange::Different);
        }
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
        item.property_expression("item")
            .chain_property::<row::ProcessRow>(property)
            .bind(&label, "label", gtk::Widget::NONE);
        if property != "name" {
            item.set_child(Some(&label));
            return;
        }
        // The name column also shows the app's icon.
        let icon = gtk::Image::builder().pixel_size(24).build();
        item.property_expression("item")
            .chain_property::<row::ProcessRow>("icon")
            .bind(&icon, "gicon", gtk::Widget::NONE);
        let cell = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        cell.append(&icon);
        cell.append(&label);
        item.set_child(Some(&cell));
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
fn confirm_end(parent: &gtk::Widget, toasts: &adw::ToastOverlay, process: &row::ProcessRow) {
    let dialog = adw::AlertDialog::new(
        Some(&format::fmt(
            "End “{name}”?",
            &[("name", &process.name())],
        )),
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

    let (pid, start_ticks) = process.key();
    let toasts = toasts.clone();
    let source = ProcessSource::detect(&SysRoot::from_env());
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |response| {
        let force = match response.as_str() {
            "end" => false,
            "kill" => true,
            _ => return,
        };
        glib::spawn_future_local(async move {
            let result =
                gio::spawn_blocking(move || send_signal(source, pid, start_ticks, force)).await;
            let message = match result {
                Ok(Ok(())) => return,
                Ok(Err(e)) if e.kind() == io::ErrorKind::PermissionDenied => {
                    String::from("You can only end your own processes")
                }
                Ok(Err(e)) if e.kind() == io::ErrorKind::NotFound => {
                    String::from("The process has already ended")
                }
                Ok(Err(error)) => {
                    tracing::warn!(pid, %error, "cannot end process");
                    String::from("Could not end the process")
                }
                Err(_) => String::from("Could not end the process"),
            };
            toasts.add_toast(adw::Toast::new(&message));
        });
    });
}

/// `CwProcessRow`: one table row. Numbers are kept for sorting, text for
/// display; both are set only when they change.
mod row {
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
