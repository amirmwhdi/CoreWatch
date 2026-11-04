//! Graphics card usage per process, from DRM fdinfo.
//!
//! Every open file of a GPU driver that follows the kernel's DRM usage
//! stats (amdgpu, i915, xe, msm, panfrost, v3d, …) has
//! `/proc/<pid>/fdinfo/<fd>` lines like
//!
//! ```text
//! drm-driver:     amdgpu
//! drm-client-id:  42
//! drm-engine-gfx: 123456789 ns
//! drm-cycles-rcs: 5000           (xe: busy cycles …
//! drm-total-cycles-rcs: 9000     …  out of all cycles)
//! ```
//!
//! The busy time of each engine between two reads, divided by the time that
//! passed, is how busy the process kept that engine; the process's usage is
//! its busiest engine. Which descriptors are GPU files is found by reading
//! the `/proc/<pid>/fd` links, only every few scans; in between only those
//! few fdinfo files are read. Without root only the user's own processes can
//! be read. The proprietary NVIDIA driver is read through NVML instead (see
//! `gpu::nvml`).

use std::collections::HashMap;
use std::time::Instant;

/// Scans between two searches for GPU descriptors.
pub const FIND_EVERY: u32 = 5;

/// One GPU client's counters, by engine.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ClientCounters {
    /// `drm-client-id`: the same client open through several descriptors
    /// (or processes) is counted once per process.
    pub client: Option<u64>,
    /// Busy nanoseconds per engine, divided by its capacity (engines with
    /// several instances).
    pub busy_ns: HashMap<String, f64>,
    /// xe: busy and total cycles per engine.
    pub cycles: HashMap<String, (u64, u64)>,
}

/// Parse one fdinfo file. `None` for a descriptor that is not a GPU client.
pub fn parse_fdinfo(text: &str) -> Option<ClientCounters> {
    let mut counters = ClientCounters::default();
    let mut capacity: HashMap<&str, f64> = HashMap::new();
    let mut busy: HashMap<&str, u64> = HashMap::new();
    let mut cycles: HashMap<&str, u64> = HashMap::new();
    let mut total: HashMap<&str, u64> = HashMap::new();
    let mut is_drm = false;
    for line in text.lines() {
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        let number = || value.split_ascii_whitespace().next()?.parse::<u64>().ok();
        if key == "drm-driver" {
            is_drm = true;
        } else if key == "drm-client-id" {
            counters.client = number();
        } else if let Some(engine) = key.strip_prefix("drm-engine-capacity-") {
            if let Some(n) = number().filter(|n| *n > 0) {
                capacity.insert(engine, n as f64);
            }
        } else if let Some(engine) = key.strip_prefix("drm-engine-") {
            if value.ends_with("ns") {
                if let Some(ns) = number() {
                    busy.insert(engine, ns);
                }
            }
        } else if let Some(engine) = key.strip_prefix("drm-total-cycles-") {
            if let Some(n) = number() {
                total.insert(engine, n);
            }
        } else if let Some(engine) = key.strip_prefix("drm-cycles-") {
            if let Some(n) = number() {
                cycles.insert(engine, n);
            }
        }
    }
    if !is_drm {
        return None;
    }
    counters.busy_ns = busy
        .into_iter()
        .map(|(engine, ns)| {
            let instances = capacity.get(engine).copied().unwrap_or(1.0);
            (engine.to_owned(), ns as f64 / instances)
        })
        .collect();
    counters.cycles = cycles
        .into_iter()
        .filter_map(|(engine, c)| Some((engine.to_owned(), (c, *total.get(engine)?))))
        .collect();
    Some(counters)
}

/// A process's busiest engine between two readings of all its clients,
/// 0.0..=1.0. `wall_ns`: the time between the readings.
pub fn usage_between(before: &[ClientCounters], now: &[ClientCounters], wall_ns: f64) -> f32 {
    if wall_ns <= 0.0 {
        return 0.0;
    }
    let find = |list: &'_ [ClientCounters], client: &ClientCounters| -> Option<ClientCounters> {
        list.iter()
            .find(|c| c.client.is_some() && c.client == client.client)
            .cloned()
    };
    // Busy share per engine, summed over the process's clients.
    let mut engines: HashMap<&str, f64> = HashMap::new();
    for client in now {
        let Some(old) = find(before, client) else {
            continue; // a new client: no difference yet
        };
        for (engine, ns) in &client.busy_ns {
            if let Some(old_ns) = old.busy_ns.get(engine) {
                let delta = (ns - old_ns).max(0.0);
                *engines.entry(engine).or_default() += delta / wall_ns;
            }
        }
        for (engine, (busy, all)) in &client.cycles {
            if let Some((old_busy, old_all)) = old.cycles.get(engine) {
                let all = all.saturating_sub(*old_all);
                if all > 0 {
                    let share = busy.saturating_sub(*old_busy) as f64 / all as f64;
                    *engines.entry(engine).or_default() += share;
                }
            }
        }
    }
    engines
        .values()
        .copied()
        .fold(0.0, f64::max)
        .clamp(0.0, 1.0) as f32
}

/// Usage per process, kept between scans.
#[derive(Default)]
pub struct GpuTracker {
    /// GPU descriptors by pid, from the last search.
    pub fds: HashMap<u32, Vec<u32>>,
    pub find_countdown: u32,
    /// Last reading per pid.
    last: HashMap<u32, (Vec<ClientCounters>, Instant)>,
}

impl GpuTracker {
    /// Forget everything, while the process list is not shown.
    pub fn pause(&mut self) {
        self.fds.clear();
        self.last.clear();
        self.find_countdown = 0;
    }

    /// Record a reading of `pid`'s clients and return its usage since the
    /// last one; `None` on the first reading.
    pub fn usage(&mut self, pid: u32, clients: Vec<ClientCounters>, now: Instant) -> Option<f32> {
        let before = self.last.insert(pid, (clients, now));
        let (old, then) = before?;
        let (clients, _) = self.last.get(&pid)?;
        let wall_ns = now.saturating_duration_since(then).as_nanos() as f64;
        Some(usage_between(&old, clients, wall_ns))
    }

    /// Drop the readings of processes that are gone.
    pub fn retain(&mut self, alive: impl Fn(u32) -> bool) {
        self.last.retain(|pid, _| alive(*pid));
        self.fds.retain(|pid, _| alive(*pid));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const AMDGPU: &str = "pos:\t0\nflags:\t02100002\nmnt_id:\t26\n\
        drm-driver:\tamdgpu\ndrm-client-id:\t7\ndrm-pdev:\t0000:01:00.0\n\
        drm-memory-vram:\t1024 KiB\ndrm-engine-gfx:\t1000000000 ns\n\
        drm-engine-compute:\t0 ns\n";
    const XE: &str = "drm-driver:\txe\ndrm-client-id:\t3\n\
        drm-cycles-rcs:\t1000\ndrm-total-cycles-rcs:\t10000\n\
        drm-cycles-vcs:\t0\ndrm-total-cycles-vcs:\t10000\n";

    #[test]
    fn reads_engine_time_and_cycles() {
        let a = parse_fdinfo(AMDGPU).unwrap();
        assert_eq!(a.client, Some(7));
        assert_eq!(a.busy_ns["gfx"], 1e9);
        // "drm-engine-capacity-*" is a capacity, not an engine.
        assert!(!a.busy_ns.contains_key("capacity-gfx"));
        let x = parse_fdinfo(XE).unwrap();
        assert_eq!(x.cycles["rcs"], (1000, 10000));
        // Not a GPU file at all.
        assert_eq!(parse_fdinfo("pos:\t0\nflags:\t02\n"), None);
    }

    #[test]
    fn usage_is_the_busiest_engine() {
        let before = parse_fdinfo(AMDGPU).unwrap();
        let mut now = before.clone();
        // 0.25 s of gfx work in 0.5 s: 50%.
        *now.busy_ns.get_mut("gfx").unwrap() += 250_000_000.0;
        let usage = usage_between(&[before], &[now], 500_000_000.0);
        assert!((usage - 0.5).abs() < 1e-6);
    }

    #[test]
    fn xe_usage_comes_from_cycles() {
        let before = parse_fdinfo(XE).unwrap();
        let mut now = before.clone();
        now.cycles.insert("rcs".into(), (1000 + 300, 10000 + 1000));
        let usage = usage_between(&[before], &[now], 1e9);
        assert!((usage - 0.3).abs() < 1e-6);
    }

    #[test]
    fn engines_with_several_instances_are_divided() {
        let text = "drm-driver:\ti915\ndrm-client-id:\t1\n\
            drm-engine-video:\t2000000000 ns\ndrm-engine-capacity-video:\t2\n";
        assert_eq!(parse_fdinfo(text).unwrap().busy_ns["video"], 1e9);
    }

    #[test]
    fn a_new_client_has_no_usage_yet() {
        let now = parse_fdinfo(AMDGPU).unwrap();
        assert_eq!(usage_between(&[], &[now], 1e9), 0.0);
    }
}
