//! `CwProcessRow`: one table row. Numbers are kept for sorting, text for
//! display; both are set only when they change.

use super::KERNEL;
use crate::app_icons::AppIcons;
use crate::format;
use adw::prelude::*;
use corewatch_core::process::ProcessInfo;
use glib::subclass::prelude::*;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::sync::Arc;

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
    pub fn update(&self, info: &ProcessInfo, gpu: Option<f32>, icons: &AppIcons, relabel: bool) {
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
