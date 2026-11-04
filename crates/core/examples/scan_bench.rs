//! Time the process scan: `cargo run --release -p corewatch-core --example
//! scan_bench [rounds]`. Prints the number of processes and the average
//! time of one scan of /proc.

use corewatch_core::process::{ProcessCollector, ProcessSource};
use corewatch_core::{Collector, Snapshot, SysRoot};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

static ACTIVE: AtomicBool = AtomicBool::new(true);

fn main() {
    let rounds: u32 = std::env::args()
        .nth(1)
        .and_then(|n| n.parse().ok())
        .unwrap_or(200);
    let root = SysRoot::host();
    let mut collector =
        ProcessCollector::new(&root, ProcessSource::Direct, &ACTIVE).expect("process collector");
    let mut snapshot = Snapshot::new();
    // Warm up the page cache and the collector's own state.
    for _ in 0..5 {
        collector.collect(&root, &mut snapshot).expect("scan");
    }
    let started = Instant::now();
    for _ in 0..rounds {
        collector.collect(&root, &mut snapshot).expect("scan");
    }
    let each = started.elapsed() / rounds;
    let count = snapshot.processes.map_or(0, |p| p.processes.len());
    println!("{count} processes, {each:?} per scan");
}
