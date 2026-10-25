//! Print every resource once per second, without any GUI.
//!
//!     cargo run -p corewatch-core --example print
//!     COREWATCH_SYSROOT=crates/core/tests/fixtures/basic cargo run -p corewatch-core --example print

use corewatch_core::{
    battery::BatteryCollector, cpu::CpuCollector, disk::DiskCollector, gpu::GpuCollector,
    memory::MemoryCollector, network::NetworkCollector, process::ProcessCollector,
    process::ProcessSource, sensors::SensorCollector,
};
use corewatch_core::{Sampler, SysRoot};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

fn main() {
    let root = SysRoot::from_env();
    let mut sampler = Sampler::new(root.clone(), Duration::from_secs(1));
    match CpuCollector::new(&root) {
        Ok(cpu) => sampler.add(Box::new(cpu)),
        Err(e) => eprintln!("cpu disabled: {e}"),
    }
    sampler.add(Box::new(MemoryCollector));
    match DiskCollector::new(&root) {
        Ok(disk) => sampler.add(Box::new(disk)),
        Err(e) => eprintln!("disk disabled: {e}"),
    }
    match GpuCollector::new(&root) {
        Ok(gpu) => sampler.add(Box::new(gpu)),
        Err(e) => eprintln!("gpu disabled: {e}"),
    }
    match BatteryCollector::new(&root) {
        Ok(bat) => sampler.add(Box::new(bat)),
        Err(e) => eprintln!("battery disabled: {e}"),
    }

    match NetworkCollector::new(&root) {
        Ok(net) => sampler.add(Box::new(net)),
        Err(e) => eprintln!("network disabled: {e}"),
    }
    match SensorCollector::new(&root) {
        Ok(sensors) => sampler.add(Box::new(sensors)),
        Err(e) => eprintln!("sensors disabled: {e}"),
    }
    static ACTIVE: AtomicBool = AtomicBool::new(true);
    match ProcessCollector::new(&root, ProcessSource::detect(&root), &ACTIVE) {
        Ok(processes) => sampler.add(Box::new(processes)),
        Err(e) => eprintln!("processes disabled: {e}"),
    }

    let (tx, rx) = async_channel::bounded(1);
    let _handle = match sampler.spawn(tx) {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("cannot start sampler: {e}");
            return;
        }
    };

    for _ in 0..5 {
        let Ok(snap) = rx.recv_blocking() else { break };
        if let Some(cpu) = &snap.cpu {
            let cores: Vec<String> = cpu
                .cores
                .iter()
                .map(|c| format!("{:.0}", c.usage * 100.0))
                .collect();
            println!(
                "CPU {:>3.0}%  [{}]  {}",
                cpu.usage * 100.0,
                cores.join(" "),
                cpu.info.model
            );
        }
        if let Some(mem) = &snap.memory {
            println!(
                "MEM {:>3.0}%  {} / {} MiB, swap {} MiB",
                mem.used_fraction() * 100.0,
                mem.used / 1024 / 1024,
                mem.total / 1024 / 1024,
                mem.swap_used / 1024 / 1024
            );
        }
        if let Some(gpu) = &snap.gpu {
            for g in &gpu.devices {
                let usage_str = g.usage.map_or("-".into(), |u| format!("{:.0}%", u * 100.0));
                let freq_str = g.freq_mhz.map_or("-".into(), |f| format!("{f} MHz"));
                println!(
                    "GPU {:>3}  {} ({:?})  clock {}  temp {:?}",
                    usage_str, g.name, g.vendor, freq_str, g.temp_c
                );
            }
        }
        if let Some(bat) = &snap.battery {
            for b in &bat.batteries {
                let p_str = b.power_w.map_or("-".into(), |w| format!("{w:.1} W"));
                println!(
                    "BAT {:>3.0}%  {:?}  rate {}  model {}",
                    b.capacity * 100.0,
                    b.state,
                    p_str,
                    b.model
                );
            }
        }
        if let Some(disk) = &snap.disk {
            for d in &disk.disks {
                let rate = |v: Option<u64>| v.map_or("-".into(), |b| format!("{} KiB/s", b / 1024));
                println!(
                    "DSK {:>3}  {} ({:?}, {} GiB)  read {}  write {}",
                    d.busy.map_or("-".into(), |b| format!("{:.0}%", b * 100.0)),
                    d.model,
                    d.kind,
                    d.size / (1024 * 1024 * 1024),
                    rate(d.read_bps),
                    rate(d.write_bps),
                );
            }
            for f in &disk.filesystems {
                println!(
                    "FS  {:>3.0}%  {} ({}, {}) {} / {} GiB",
                    f.space.used_fraction() * 100.0,
                    f.mount_point.display(),
                    f.fs_type,
                    f.source,
                    f.space.used / (1024 * 1024 * 1024),
                    f.space.total / (1024 * 1024 * 1024),
                );
            }
        }
        if let Some(net) = &snap.network {
            for i in &net.interfaces {
                let rate = |v: Option<u64>| v.map_or("-".into(), |b| format!("{} KiB/s", b / 1024));
                println!(
                    "NET {} ({:?}, {})  down {}  up {}  {:?} {:?}",
                    i.name,
                    i.kind,
                    if i.connected { "connected" } else { "off" },
                    rate(i.rx_bps),
                    rate(i.tx_bps),
                    i.addresses.ipv4,
                    i.link_mbps,
                );
            }
        }
        if let Some(sensors) = &snap.sensors {
            for c in &sensors.chips {
                let temps: Vec<String> = c
                    .temperatures
                    .iter()
                    .map(|t| format!("{}={:.0}°C", t.label.as_deref().unwrap_or("?"), t.celsius))
                    .collect();
                let fans: Vec<String> = c.fans.iter().map(|f| format!("{} RPM", f.rpm)).collect();
                println!(
                    "SEN {} ({:?})  {}  {}",
                    c.driver,
                    c.kind,
                    temps.join(" "),
                    fans.join(" ")
                );
            }
        }
        if let Some(p) = &snap.processes {
            let mut top: Vec<_> = p.processes.iter().collect();
            top.sort_by(|a, b| b.cpu.total_cmp(&a.cpu));
            for t in top.iter().take(3) {
                println!(
                    "PRC {:>3.0}%  {} ({}, {})  {} MiB",
                    t.cpu * 100.0,
                    t.name,
                    t.pid,
                    t.user,
                    t.memory / 1024 / 1024
                );
            }
            println!("PRC {} processes", p.processes.len());
        }
    }
}
