//! Print CPU, memory and disks once per second, without any GUI.
//!
//!     cargo run -p corewatch-core --example print
//!     COREWATCH_SYSROOT=crates/core/tests/fixtures/basic cargo run -p corewatch-core --example print

use corewatch_core::{
    cpu::CpuCollector, disk::DiskCollector, gpu::GpuCollector, memory::MemoryCollector,
};
use corewatch_core::{Sampler, SysRoot};
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
    }
}
