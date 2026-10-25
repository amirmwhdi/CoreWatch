//! Processes page: every process in a sortable, searchable table, with a
//! button to end the selected one.
//!
//! Rows are GObjects kept per pid and updated in place, so a tick changes a
//! few properties instead of rebuilding the table, and only the rows on
//! screen redraw.

use super::{Module, ResourcePage};
use crate::i18n::{self, gettext, ngettext};
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
    store: gio::ListStore,
    rows: RefCell<HashMap<u32, row::ProcessRow>>,
    columns: gtk::ColumnView,
    generation: Cell<u64>,
    count: Cell<usize>,
}

impl ProcessesPage {
    fn new() -> Self {
        let view = view::ProcessesView::new();
        let ui = view.imp();
        let columns = ui.columns.get();
        let store = gio::ListStore::new::<row::ProcessRow>();

        // Search by name, optionally only the user's own processes.
        let name_expr = gtk::PropertyExpression::new(
            row::ProcessRow::static_type(),
            gtk::Expression::NONE,
            "name",
        );
        let search = gtk::StringFilter::builder()
            .expression(&name_expr)
            .match_mode(gtk::StringFilterMatchMode::Substring)
            .ignore_case(true)
            .build();
        ui.search
            .bind_property("text", &search, "search")
            .sync_create()
            .build();
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

        let name = add_column(&columns, &gettext("Name"), "name", None, true);
        name.set_sorter(Some(&gtk::StringSorter::new(Some(name_expr))));
        add_column(&columns, &gettext("User"), "user", Some("user"), false);
        let cpu = add_column(&columns, &gettext("CPU"), "cpu-text", Some("cpu"), false);
        add_column(
            &columns,
            &gettext("Memory"),
            "memory-text",
            Some("memory"),
            false,
        );
        add_column(&columns, &gettext("ID"), "pid-text", Some("pid"), false);
        columns.sort_by_column(Some(&cpu), gtk::SortType::Descending);

        // The table only needs fresh data while it is on screen.
        view.connect_map(|_| ACTIVE.store(true, Ordering::Relaxed));
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

        Self {
            store,
            rows: RefCell::default(),
            columns,
            generation: Cell::new(0),
            count: Cell::new(0),
            root: view.upcast(),
        }
    }

    fn sync(&self, processes: &[ProcessInfo]) {
        let mut rows = self.rows.borrow_mut();
        let mut added = Vec::new();
        let mut alive = HashSet::with_capacity(processes.len());
        for info in processes {
            alive.insert(info.pid);
            match rows.get(&info.pid) {
                Some(row) => row.update(info),
                None => {
                    let row = row::ProcessRow::new(info);
                    added.push(row.clone());
                    rows.insert(info.pid, row);
                }
            }
        }
        if rows.len() > alive.len() {
            rows.retain(|pid, _| alive.contains(pid));
            self.store.retain(|item| {
                item.downcast_ref::<row::ProcessRow>()
                    .is_some_and(|r| alive.contains(&r.pid()))
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
        gettext("Processes")
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
        self.sync(&sample.processes);
    }

    fn has_data(&self, snapshot: &Snapshot) -> bool {
        snapshot.processes.is_some()
    }

    fn summary(&self, snapshot: &Snapshot) -> Option<String> {
        let n = snapshot.processes.as_ref()?.processes.len();
        Some(i18n::fmt(
            &ngettext("{n} process", "{n} processes", i18n::count(n)),
            &[("n", &n)],
        ))
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
        item.set_child(Some(&label));
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
        Some(&i18n::fmt(
            &gettext("End “{name}”?"),
            &[("name", &process.name())],
        )),
        Some(&gettext(
            "Unsaved work in this program may be lost. Force Stop ends it at once, without letting it clean up.",
        )),
    );
    dialog.add_responses(&[
        ("cancel", &gettext("_Cancel")),
        ("kill", &gettext("_Force Stop")),
        ("end", &gettext("_End Process")),
    ]);
    dialog.set_response_appearance("kill", adw::ResponseAppearance::Destructive);
    dialog.set_response_appearance("end", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("end"));
    dialog.set_close_response("cancel");

    let pid = process.pid();
    let toasts = toasts.clone();
    let source = ProcessSource::detect(&SysRoot::from_env());
    dialog.choose(Some(parent), gio::Cancellable::NONE, move |response| {
        let force = match response.as_str() {
            "end" => false,
            "kill" => true,
            _ => return,
        };
        glib::spawn_future_local(async move {
            let result = gio::spawn_blocking(move || send_signal(source, pid, force)).await;
            let message = match result {
                Ok(Ok(())) => return,
                Ok(Err(e)) if e.kind() == io::ErrorKind::PermissionDenied => {
                    gettext("You can only end your own processes")
                }
                Ok(Err(e)) if e.kind() == io::ErrorKind::NotFound => {
                    gettext("The process has already ended")
                }
                Ok(Err(error)) => {
                    tracing::warn!(pid, %error, "cannot end process");
                    gettext("Could not end the process")
                }
                Err(_) => gettext("Could not end the process"),
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
            #[property(get)]
            pub pid_text: RefCell<String>,
            /// `u32::MAX` when unknown.
            #[property(get)]
            pub uid: Cell<u32>,
            #[property(get)]
            pub name: RefCell<String>,
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
        pub fn new(info: &ProcessInfo) -> Self {
            let row: Self = glib::Object::new();
            let imp = row.imp();
            imp.pid.set(info.pid);
            *imp.pid_text.borrow_mut() = info.pid.to_string();
            imp.uid.set(info.uid.unwrap_or(u32::MAX));
            *imp.user.borrow_mut() = info.user.to_string();
            *imp.name.borrow_mut() = info.name.to_string();
            imp.cpu.set(info.cpu);
            *imp.cpu_text.borrow_mut() = cpu_text(info.cpu);
            imp.memory.set(info.memory);
            *imp.memory_text.borrow_mut() = i18n::size(info.memory);
            row
        }

        pub fn update(&self, info: &ProcessInfo) {
            let imp = self.imp();
            if *imp.name.borrow() != *info.name {
                *imp.name.borrow_mut() = info.name.to_string();
                self.notify_name();
            }
            if imp.cpu.replace(info.cpu) != info.cpu {
                self.notify_cpu();
                set_text(&imp.cpu_text, cpu_text(info.cpu), || self.notify_cpu_text());
            }
            if imp.memory.replace(info.memory) != info.memory {
                self.notify_memory();
                set_text(&imp.memory_text, i18n::size(info.memory), || {
                    self.notify_memory_text()
                });
            }
        }
    }

    fn set_text(cell: &RefCell<String>, text: String, notify: impl FnOnce()) {
        if *cell.borrow() != text {
            *cell.borrow_mut() = text;
            notify();
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
        // Translators: a percentage, e.g. "37%"
        i18n::fmt(&gettext("{value}%"), &[("value", &value)])
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
