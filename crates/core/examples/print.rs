//! Print CPU, memory and disks once per second, without any GUI.
//!
//!     cargo run -p corewatch-core --example print
//!     COREWATCH_SYSROOT=crates/core/tests/fixtures/basic cargo run -p corewatch-core --example print

use corewatch_core::cpu::CpuCollector;
use corewatch_core::disk::DiskCollector;
use corewatch_core::memory::MemoryCollector;
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
        if let Some(disk) = &snap.disk {
            for d in &disk.disks {
                let rate = |v: Option<u64>| v.map_or("-".into(), |b| format!("{} KB/s", b / 1000));
                println!(
                    "DSK {:>3}  {} ({:?}, {} GB)  read {}  write {}",
                    d.busy.map_or("-".into(), |b| format!("{:.0}%", b * 100.0)),
                    d.model,
                    d.kind,
                    d.size / 1_000_000_000,
                    rate(d.read_bps),
                    rate(d.write_bps),
                );
            }
            for f in &disk.filesystems {
                println!(
                    "FS  {:>3.0}%  {} ({}, {}) {} / {} GB",
                    f.space.used_fraction() * 100.0,
                    f.mount_point.display(),
                    f.fs_type,
                    f.source,
                    f.space.used / 1_000_000_000,
                    f.space.total / 1_000_000_000,
                );
            }
        }
    }
}
