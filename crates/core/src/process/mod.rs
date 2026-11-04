//! Processes module: every running process with its CPU share and memory.
//!
//! Scanning `/proc` costs a few hundred file reads, so the collector scans
//! every tick only while someone looks at the list (the `active` flag the
//! page sets), and every [`IDLE_EVERY`] ticks otherwise, which keeps the
//! process count in the sidebar current.

pub mod cgroup;
pub mod gpu;
pub mod net;
mod parse;
mod source;
mod users;

pub use parse::{parse_passwd, parse_proc_stat, parse_statm, ProcStat};
pub use source::{current_uid, send_signal, ProcessSource};

use crate::cpu::{parse_stat, CpuTimes};
use crate::{CollectError, Collector, Snapshot, SysRoot};
use net::NetTracker;
use source::Reader;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use users::UserNames;

/// Ticks between two scans while the process list is not shown.
pub const IDLE_EVERY: u32 = 10;

#[derive(Debug, Clone, PartialEq)]
pub struct ProcessInfo {
    pub pid: u32,
    /// The program's name: the kernel's 15-byte name, completed from the
    /// command line when the kernel cut it short.
    pub name: Arc<str>,
    /// User name, or the uid as text when it has no name.
    pub user: Arc<str>,
    pub uid: Option<u32>,
    /// Share of the whole machine's CPU time since the previous scan,
    /// 0.0..=1.0; 0.0 on the first scan after the process appeared.
    pub cpu: f32,
    /// Private resident memory in bytes.
    pub memory: u64,
    pub threads: u32,
    pub state: char,
    /// Start time after boot, in clock ticks. With the pid it names one
    /// process even after the pid is reused; [`send_signal`] checks it.
    pub start_ticks: u64,
    /// Bytes per second received and sent over TCP while the list is shown;
    /// `None` when unknown (another user's process, or the list hidden).
    pub download: Option<u64>,
    pub upload: Option<u64>,
    /// The app (its ID, the name of its `.desktop` file) or service it
    /// belongs to; `None` for kernel threads. See [`cgroup::group`].
    pub group: Option<Arc<str>>,
    /// How busy the process keeps the graphics card (its busiest engine),
    /// 0.0..=1.0, from DRM fdinfo while the list is shown; `None` when
    /// unknown (another user's process, inside Flatpak, or the list hidden).
    pub gpu: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProcessSample {
    /// Every process, in no particular order; the page sorts.
    pub processes: Arc<[ProcessInfo]>,
    /// Counts up with every scan, so the page can skip unchanged samples.
    pub generation: u64,
}

/// What is kept per process between scans.
struct Known {
    start_ticks: u64,
    cpu_ticks: u64,
    name: Arc<str>,
    group: Option<Arc<str>>,
}

pub struct ProcessCollector {
    reader: Reader,
    active: &'static AtomicBool,
    users: UserNames,
    known: HashMap<u32, Known>,
    prev_cpu: Option<CpuTimes>,
    page_bytes: u64,
    last: ProcessSample,
    idle_countdown: u32,
    net: NetTracker,
    gpu: gpu::GpuTracker,
    my_uid: u32,
}

impl ProcessCollector {
    /// Scans once, so the first sample already holds the list. `active` tells
    /// the collector whether the list is on screen.
    pub fn new(
        root: &SysRoot,
        source: ProcessSource,
        active: &'static AtomicBool,
    ) -> Result<Self, CollectError> {
        let mut reader = Reader::new(source);
        let users = UserNames::new(&reader.passwd(), root, source);
        let mut collector = Self {
            reader,
            active,
            users,
            known: HashMap::new(),
            prev_cpu: None,
            page_bytes: u64::try_from(rustix::param::page_size()).unwrap_or(4096),
            last: ProcessSample::default(),
            idle_countdown: 0,
            net: NetTracker::default(),
            gpu: gpu::GpuTracker::default(),
            my_uid: current_uid(),
        };
        collector.scan(root)?;
        Ok(collector)
    }

    /// Speeds by pid while the list is shown; `None` otherwise or when the
    /// kernel gives no socket counters.
    fn net_rates(&mut self, root: &SysRoot) -> Option<HashMap<u32, (u64, u64)>> {
        if !self.active.load(Ordering::Relaxed) {
            self.net.pause();
            return None;
        }
        let sockets = match net::tcp_sockets() {
            Ok(sockets) => sockets,
            Err(e) => {
                tracing::debug!("socket counters: {e}");
                return None;
            }
        };
        if self.net.owners_countdown == 0 && self.net.needs_owners(&sockets) {
            let owners = self.reader.socket_owners(root);
            self.net.set_owners(owners, &sockets);
            self.net.owners_countdown = net::OWNERS_EVERY;
        }
        self.net.owners_countdown = self.net.owners_countdown.saturating_sub(1);
        Some(self.net.rates(sockets))
    }

    /// GPU usage by pid while the list is shown; `None` otherwise or when
    /// it cannot be read. `own`: the processes whose descriptors can be read.
    fn gpu_usage(&mut self, root: &SysRoot, own: &[u32]) -> Option<HashMap<u32, f32>> {
        if !self.active.load(Ordering::Relaxed) || !self.reader.reads_gpu() {
            self.gpu.pause();
            return None;
        }
        if self.gpu.find_countdown == 0 {
            self.gpu.fds = self.reader.gpu_fds(root, own);
            self.gpu.find_countdown = gpu::FIND_EVERY;
        }
        self.gpu.find_countdown -= 1;
        let now = std::time::Instant::now();
        let mut usage = HashMap::new();
        let holders: Vec<(u32, Vec<u32>)> =
            self.gpu.fds.iter().map(|(p, f)| (*p, f.clone())).collect();
        for (pid, fds) in holders {
            let mut clients: Vec<gpu::ClientCounters> = Vec::new();
            for fd in fds {
                let Some(client) = self
                    .reader
                    .fdinfo(root, pid, fd)
                    .and_then(|text| gpu::parse_fdinfo(&text))
                else {
                    continue;
                };
                // One client open through several descriptors counts once.
                if client.client.is_none() || !clients.iter().any(|c| c.client == client.client) {
                    clients.push(client);
                }
            }
            if let Some(share) = self.gpu.usage(pid, clients, now) {
                usage.insert(pid, share);
            }
        }
        let own: std::collections::HashSet<u32> = own.iter().copied().collect();
        self.gpu.retain(|pid| own.contains(&pid));
        Some(usage)
    }

    fn scan(&mut self, root: &SysRoot) -> Result<(), CollectError> {
        let scan = self.reader.scan(root)?;
        let rates = self.net_rates(root);
        let own: Vec<u32> = scan
            .processes
            .iter()
            .filter(|p| self.my_uid == 0 || p.uid == Some(self.my_uid))
            .map(|p| p.stat.pid)
            .collect();
        let gpu_usage = self.gpu_usage(root, &own);
        let cpu_now = scan
            .cpu_line
            .as_deref()
            .and_then(|line| parse_stat(line).ok())
            .map(|s| s.total);
        let machine_ticks = match (cpu_now, self.prev_cpu) {
            (Some(now), Some(prev)) => now.total().saturating_sub(prev.total()),
            _ => 0,
        };

        let mut known = HashMap::with_capacity(scan.processes.len());
        let processes: Vec<ProcessInfo> = scan
            .processes
            .into_iter()
            .map(|raw| {
                let stat = raw.stat;
                // Same pid and start time: the same process as last scan.
                let before = self
                    .known
                    .remove(&stat.pid)
                    .filter(|k| k.start_ticks == stat.start_ticks);
                let cpu = match &before {
                    Some(k) if machine_ticks > 0 => {
                        (stat.cpu_ticks.saturating_sub(k.cpu_ticks) as f64 / machine_ticks as f64)
                            .clamp(0.0, 1.0) as f32
                    }
                    _ => 0.0,
                };
                let (name, group) = match before {
                    Some(k) => (k.name, k.group),
                    None => {
                        let group = raw
                            .cgroup
                            .clone()
                            .or_else(|| self.reader.cgroup(root, stat.pid))
                            .and_then(|text| cgroup::group(&text))
                            .map(Arc::from);
                        (full_name(root, &self.reader, &stat), group)
                    }
                };
                known.insert(
                    stat.pid,
                    Known {
                        start_ticks: stat.start_ticks,
                        cpu_ticks: stat.cpu_ticks,
                        name: Arc::clone(&name),
                        group: group.clone(),
                    },
                );
                let user = match raw.uid {
                    Some(uid) => self.users.get(uid),
                    None => Arc::from("?"),
                };
                let (download, upload) = match &rates {
                    // Own processes (all of them for root) can be matched to
                    // sockets; others show no speed rather than a wrong zero.
                    Some(rates) if raw.uid == Some(self.my_uid) || self.my_uid == 0 => {
                        let (rx, tx) = rates.get(&stat.pid).copied().unwrap_or_default();
                        (Some(rx), Some(tx))
                    }
                    _ => (None, None),
                };
                let gpu = gpu_usage.as_ref().and_then(|usage| {
                    let readable = self.my_uid == 0 || raw.uid == Some(self.my_uid);
                    readable.then(|| usage.get(&stat.pid).copied().unwrap_or(0.0))
                });
                ProcessInfo {
                    pid: stat.pid,
                    name,
                    user,
                    uid: raw.uid,
                    cpu,
                    memory: raw.private_pages.saturating_mul(self.page_bytes),
                    threads: stat.threads,
                    state: stat.state,
                    start_ticks: stat.start_ticks,
                    download,
                    upload,
                    gpu,
                    group,
                }
            })
            .collect();

        self.known = known;
        self.prev_cpu = cpu_now;
        self.last = ProcessSample {
            processes: processes.into(),
            generation: self.last.generation + 1,
        };
        Ok(())
    }
}

/// The kernel cuts names at 15 bytes. For such names, outside Flatpak, the
/// first word of the command line gives the whole name when it starts the
/// same way (`gnome-shell-calendar-server`, not `gnome-shell-cal`).
fn full_name(root: &SysRoot, reader: &Reader, stat: &ProcStat) -> Arc<str> {
    if stat.comm.len() >= 15 && matches!(reader, Reader::Direct { .. }) {
        if let Ok(cmdline) = root.read_proc(&format!("{}/cmdline", stat.pid)) {
            let argv0 = cmdline.split('\0').next().unwrap_or_default();
            let base = argv0.rsplit('/').next().unwrap_or_default();
            if base.starts_with(&stat.comm) {
                return Arc::from(base);
            }
        }
    }
    Arc::from(stat.comm.as_str())
}

impl Collector for ProcessCollector {
    fn name(&self) -> &'static str {
        "processes"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        if self.active.load(Ordering::Relaxed) || self.idle_countdown == 0 {
            self.scan(root)?;
            self.idle_countdown = IDLE_EVERY;
        }
        self.idle_countdown = self.idle_countdown.saturating_sub(1);
        out.processes = Some(self.last.clone());
        Ok(())
    }
}
