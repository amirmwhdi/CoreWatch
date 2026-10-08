//! Print CPU and memory once per second, without any GUI.
//!
//!     cargo run -p corewatch-core --example print
//!     COREWATCH_SYSROOT=crates/core/tests/fixtures/basic cargo run -p corewatch-core --example print

use corewatch_core::cpu::CpuCollector;
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
    }
}
