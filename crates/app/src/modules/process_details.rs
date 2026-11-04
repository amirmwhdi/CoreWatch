//! The details window of one process: what it is, what it does right now
//! (CPU, memory, GPU, disk, network connections), and controls to suspend
//! it, lower its priority or end it.
//!
//! It refreshes every second while open. The live numbers the process list
//! already has (CPU, memory, GPU) come from its row; the rest is read for
//! this one process only (`corewatch_core::process::details`).

use super::processes::{confirm_end, row::ProcessRow};
use crate::format;
use adw::prelude::*;
use corewatch_core::process::net::{tcp_connections, Connection, TcpState};
use corewatch_core::process::{
    details, send_signal, set_priority, Details, ProcessSignal, ProcessSource,
};
use corewatch_core::SysRoot;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::{Duration, Instant};

/// Niceness levels offered, with their names. Without root a process can
/// only be made nicer, so the list starts at "Normal".
const PRIORITIES: [(&str, i32); 4] = [("Normal", 0), ("Low", 5), ("Very Low", 10), ("Lowest", 19)];

/// Everything the window updates.
struct Window {
    pid: u32,
    dialog: adw::Dialog,
    toasts: adw::ToastOverlay,
    banner: adw::Banner,
    command: adw::ActionRow,
    executable: adw::ActionRow,
    ids: adw::ActionRow,
    state: adw::ActionRow,
    started: adw::ActionRow,
    threads: adw::ActionRow,
    priority: adw::ComboRow,
    cpu: adw::ActionRow,
    memory: adw::ActionRow,
    gpu: adw::ActionRow,
    disk_read: adw::ActionRow,
    disk_write: adw::ActionRow,
    connections: adw::PreferencesGroup,
    connection_rows: RefCell<Vec<adw::ActionRow>>,
    shown_connections: RefCell<Option<Vec<Connection>>>,
    suspend: gtk::Button,
    end: gtk::Button,
    /// The last disk counters and when they were read, for the speeds.
    last_io: Cell<Option<((u64, u64), Instant)>>,
    /// The nice value shown, so a refresh does not undo a change in progress.
    nice: Cell<i32>,
    /// Set while the priority row is being filled, not changed by the user.
    filling: Cell<bool>,
    suspended: Cell<bool>,
}

/// Open the details window of `process` over `parent`.
pub fn show(parent: &gtk::Widget, process: &ProcessRow) {
    let (pid, start_ticks) = process.key();
    let source = ProcessSource::detect(&SysRoot::from_env());
    let window = Rc::new(build(process));

    // Suspend / Resume.
    window.suspend.connect_clicked(glib::clone!(
        #[strong]
        window,
        move |_| {
            let signal = if window.suspended.get() {
                ProcessSignal::Resume
            } else {
                ProcessSignal::Suspend
            };
            let window = window.clone();
            glib::spawn_future_local(async move {
                let result =
                    gio::spawn_blocking(move || send_signal(source, pid, start_ticks, signal))
                        .await;
                match result {
                    Ok(Ok(())) => {
                        refresh(&window, source, pid, start_ticks).await;
                    }
                    Ok(Err(e)) => window.toast(&error_text(&e, "Could not change the process")),
                    Err(_) => window.toast("Could not change the process"),
                }
            });
        }
    ));

    // Priority.
    window.priority.connect_selected_notify(glib::clone!(
        #[strong]
        window,
        move |row| {
            if window.filling.get() {
                return;
            }
            let Some(&(_, nice)) = PRIORITIES.get(row.selected() as usize) else {
                return;
            };
            if nice == window.nice.get() {
                return;
            }
            let window = window.clone();
            glib::spawn_future_local(async move {
                let result =
                    gio::spawn_blocking(move || set_priority(source, pid, start_ticks, nice)).await;
                match result {
                    Ok(Ok(())) => window.nice.set(nice),
                    Ok(Err(e)) => {
                        window.toast(&error_text(&e, "Could not change the priority"));
                        window.fill_priority(window.nice.get());
                    }
                    Err(_) => window.fill_priority(window.nice.get()),
                }
            });
        }
    ));

    // End Process…: the same dialog as in the list.
    window.end.connect_clicked(glib::clone!(
        #[strong]
        window,
        #[strong]
        process,
        move |button| confirm_end(button.upcast_ref(), &window.toasts, &process)
    ));

    window.dialog.present(Some(parent));

    // Refresh once now, then every second until the window closes or the
    // process ends.
    let weak = window.dialog.downgrade();
    let process = process.clone();
    glib::spawn_future_local(async move {
        loop {
            if weak.upgrade().is_none() {
                break;
            }
            window.live(&process);
            if !refresh(&window, source, pid, start_ticks).await {
                break;
            }
            glib::timeout_future(Duration::from_secs(1)).await;
        }
    });
}

/// Read the process's details and show them; `false` once it has ended.
async fn refresh(window: &Window, source: ProcessSource, pid: u32, start_ticks: u64) -> bool {
    let read = gio::spawn_blocking(move || {
        let found = details(source, pid, start_ticks)?;
        // Connections only when its sockets are known and it has some.
        let connections = match &found.sockets {
            Some(sockets) if !sockets.is_empty() => {
                let all = tcp_connections().unwrap_or_default();
                Some(
                    sockets
                        .iter()
                        .filter_map(|inode| all.get(inode).cloned())
                        .collect::<Vec<_>>(),
                )
            }
            Some(_) => Some(Vec::new()),
            None => None,
        };
        Ok::<_, std::io::Error>((found, connections))
    })
    .await;
    match read {
        Ok(Ok((found, connections))) => {
            window.show_details(&found);
            window.show_connections(connections);
            true
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            window.ended();
            false
        }
        _ => true,
    }
}

fn build(process: &ProcessRow) -> Window {
    let row = |title: &str| {
        adw::ActionRow::builder()
            .title(title)
            .subtitle("…")
            .subtitle_selectable(true)
            .css_classes(["property"])
            .build()
    };
    let group = |title: &str, rows: &[&adw::ActionRow]| {
        let group = adw::PreferencesGroup::builder().title(title).build();
        for row in rows {
            group.add(*row);
        }
        group
    };

    let command = row("Command Line");
    let executable = row("Executable");
    let ids = row("Process ID");
    let state = row("State");
    let started = row("Started");
    let threads = row("Threads");
    let priority = adw::ComboRow::builder()
        .title("Priority")
        .subtitle("Low priority lets other programs run first")
        .model(&gtk::StringList::new(&PRIORITIES.map(|(name, _)| name)))
        .build();
    let process_group = group(
        "Process",
        &[&command, &executable, &ids, &state, &started, &threads],
    );
    process_group.add(&priority);

    let cpu = row("CPU");
    let memory = row("Memory");
    let gpu = row("Graphics Card");
    let disk_read = row("Disk Read");
    let disk_write = row("Disk Write");
    let activity = group("Activity", &[&cpu, &memory, &gpu, &disk_read, &disk_write]);

    let connections = adw::PreferencesGroup::builder()
        .title("Network Connections")
        .description("TCP connections of this process")
        .build();

    let suspend = gtk::Button::builder()
        .label("_Suspend")
        .use_underline(true)
        .tooltip_text("Freeze the process until it is resumed")
        .build();
    let end = gtk::Button::builder()
        .label("_End Process…")
        .use_underline(true)
        .css_classes(["destructive-action"])
        .build();
    let buttons = gtk::Box::builder()
        .spacing(12)
        .halign(gtk::Align::Center)
        .homogeneous(true)
        .build();
    buttons.append(&suspend);
    buttons.append(&end);
    let actions = adw::PreferencesGroup::new();
    actions.add(&buttons);

    let page = adw::PreferencesPage::new();
    page.add(&process_group);
    page.add(&activity);
    page.add(&connections);
    page.add(&actions);

    let banner = adw::Banner::new("This process has ended");
    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&banner);
    content.append(&page);
    let toasts = adw::ToastOverlay::new();
    toasts.set_child(Some(&content));
    let toolbar = adw::ToolbarView::builder().content(&toasts).build();
    toolbar.add_top_bar(&adw::HeaderBar::new());

    let dialog = adw::Dialog::builder()
        .title(process.name())
        .content_width(520)
        .content_height(680)
        .child(&toolbar)
        .build();

    Window {
        pid: process.pid(),
        dialog,
        toasts,
        banner,
        command,
        executable,
        ids,
        state,
        started,
        threads,
        priority,
        cpu,
        memory,
        gpu,
        disk_read,
        disk_write,
        connections,
        connection_rows: RefCell::default(),
        shown_connections: RefCell::default(),
        suspend,
        end,
        last_io: Cell::new(None),
        nice: Cell::new(0),
        filling: Cell::new(false),
        suspended: Cell::new(false),
    }
}

impl Window {
    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    /// The numbers the process list already has.
    fn live(&self, process: &ProcessRow) {
        set(&self.cpu, &process.cpu_text());
        set(&self.memory, &process.memory_text());
        let gpu = process.gpu_text();
        set(
            &self.gpu,
            match gpu.as_str() {
                "" => "Not used",
                "—" => "Not known for another user's process",
                text => text,
            },
        );
    }

    fn show_details(&self, d: &Details) {
        set(
            &self.command,
            if d.command_line.is_empty() {
                "None (a kernel thread)"
            } else {
                &d.command_line
            },
        );
        set(
            &self.executable,
            d.executable.as_deref().unwrap_or("Not readable"),
        );
        set(
            &self.ids,
            &format::fmt(
                "{pid} · parent {ppid}",
                &[("pid", &self.pid), ("ppid", &d.ppid)],
            ),
        );
        set(&self.state, state_text(d.state));
        set(&self.started, &started_text(d.started_secs));
        set(&self.threads, &d.threads.to_string());
        self.suspended.set(d.state == 'T');
        self.suspend.set_label(if d.state == 'T' {
            "_Resume"
        } else {
            "_Suspend"
        });
        if d.nice != self.nice.get() || self.priority.selected() == gtk::INVALID_LIST_POSITION {
            self.fill_priority(d.nice);
        }
        self.show_io(d.io);
    }

    /// Select the level of `nice`, or the nearest one below.
    fn fill_priority(&self, nice: i32) {
        self.filling.set(true);
        self.nice.set(nice);
        let index = PRIORITIES
            .iter()
            .rposition(|(_, level)| *level <= nice)
            .unwrap_or(0);
        self.priority.set_selected(index as u32);
        self.filling.set(false);
    }

    fn show_io(&self, io: Option<(u64, u64)>) {
        let Some(now) = io else {
            set(&self.disk_read, "Not known for another user's process");
            set(&self.disk_write, "Not known for another user's process");
            return;
        };
        let at = Instant::now();
        let speeds = self
            .last_io
            .replace(Some((now, at)))
            .and_then(|(before, then)| {
                let secs = at.duration_since(then).as_secs_f64();
                (secs > 0.0).then(|| {
                    let rate = |a: u64, b: u64| (a.saturating_sub(b) as f64 / secs) as u64;
                    (rate(now.0, before.0), rate(now.1, before.1))
                })
            });
        let text = |speed: Option<u64>, total: u64| match speed {
            Some(speed) => format::fmt(
                "{speed} · {total} in all",
                &[
                    ("speed", &format::speed(speed)),
                    ("total", &format::size(total)),
                ],
            ),
            None => format::fmt("{total} in all", &[("total", &format::size(total))]),
        };
        set(&self.disk_read, &text(speeds.map(|s| s.0), now.0));
        set(&self.disk_write, &text(speeds.map(|s| s.1), now.1));
    }

    fn show_connections(&self, connections: Option<Vec<Connection>>) {
        if *self.shown_connections.borrow() == connections {
            return;
        }
        for row in self.connection_rows.take() {
            self.connections.remove(&row);
        }
        let rows: Vec<adw::ActionRow> = match &connections {
            None => vec![note("Not readable for another user's process")],
            Some(list) if list.is_empty() => vec![note("No TCP connections")],
            Some(list) => list.iter().map(connection_row).collect(),
        };
        for row in &rows {
            self.connections.add(row);
        }
        *self.connection_rows.borrow_mut() = rows;
        *self.shown_connections.borrow_mut() = connections;
    }

    fn ended(&self) {
        self.banner.set_revealed(true);
        self.suspend.set_sensitive(false);
        self.end.set_sensitive(false);
        self.priority.set_sensitive(false);
    }
}

/// Set a row's subtitle only when it changed (keeps a text selection).
fn set(row: &adw::ActionRow, text: &str) {
    if row.subtitle().as_deref() != Some(text) {
        row.set_subtitle(text);
    }
}

fn note(text: &str) -> adw::ActionRow {
    adw::ActionRow::builder()
        .title(text)
        .css_classes(["dim-label"])
        .build()
}

/// "93.184.216.34:443", "Established · from 192.168.1.10:51234".
fn connection_row(c: &Connection) -> adw::ActionRow {
    let (title, subtitle) = match c.state {
        TcpState::Listening => (
            format::fmt("Listening on {addr}", &[("addr", &c.local)]),
            String::from("Waits for connections"),
        ),
        state => (
            c.remote.to_string(),
            format::fmt(
                "{state} · from {local}",
                &[
                    (
                        "state",
                        &match state {
                            TcpState::Established => "Connected",
                            TcpState::Connecting => "Connecting",
                            _ => "Closing",
                        },
                    ),
                    ("local", &c.local),
                ],
            ),
        ),
    };
    adw::ActionRow::builder()
        .title(title)
        .subtitle(subtitle)
        .title_selectable(true)
        .css_classes(["property"])
        .build()
}

fn state_text(state: char) -> &'static str {
    match state {
        'R' => "Running",
        'S' => "Sleeping",
        'D' => "Waiting for the disk",
        'T' | 't' => "Suspended",
        'Z' => "Ended, waiting for its parent (zombie)",
        'I' => "Idle",
        _ => "Unknown",
    }
}

/// "Today at 10:32", or "3 Nov 2026 at 10:32".
fn started_text(started_secs: f64) -> String {
    let uptime = std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|t| t.split_ascii_whitespace().next()?.parse::<f64>().ok());
    let (Some(uptime), Ok(now)) = (uptime, glib::DateTime::now_local()) else {
        return String::from("Unknown");
    };
    let Ok(at) = now.add_seconds(started_secs - uptime) else {
        return String::from("Unknown");
    };
    let same_day = at.ymd() == now.ymd();
    let pattern = if same_day {
        "Today at %H:%M"
    } else {
        "%e %b %Y at %H:%M"
    };
    at.format(pattern)
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| String::from("Unknown"))
}

fn error_text(e: &std::io::Error, otherwise: &str) -> String {
    match e.kind() {
        std::io::ErrorKind::PermissionDenied => {
            String::from("Only your own processes can be changed, and only to a lower priority")
        }
        std::io::ErrorKind::NotFound => String::from("The process has already ended"),
        _ => otherwise.to_owned(),
    }
}
