//! Real collectors against fake `/proc` and `/sys` trees.

use corewatch_core::cpu::CpuCollector;
use corewatch_core::memory::MemoryCollector;
use corewatch_core::{Collector, Sampler, Snapshot, SysRoot};
use std::time::Duration;

fn fixture(name: &str) -> SysRoot {
    SysRoot::at(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
}

#[test]
fn memory_collector_fills_its_field_only() {
    let mut snap = Snapshot::new();
    MemoryCollector
        .collect(&fixture("basic"), &mut snap)
        .unwrap();
    let m = snap.memory.unwrap();
    assert_eq!(m.total, 8_000_000 * 1024);
    assert_eq!(m.used, 2_000_000 * 1024);
    assert_eq!(m.swap_used, 0);
    assert!(snap.cpu.is_none());
}

#[test]
fn cpu_collector_reads_info_cores_and_frequency() {
    let root = fixture("basic");
    let mut collector = CpuCollector::new(&root).unwrap();
    let mut snap = Snapshot::new();
    collector.collect(&root, &mut snap).unwrap();

    let cpu = snap.cpu.unwrap();
    assert_eq!(cpu.info.model, "Fixture CPU @ 3.00GHz");
    assert_eq!(cpu.info.logical, 2);
    assert_eq!(cpu.info.physical, Some(2));
    assert_eq!(cpu.cores.len(), 2);
    // Same file read twice: no time passed, so usage is zero.
    assert_eq!(cpu.usage, 0.0);
    assert_eq!(cpu.cores[0].freq_mhz, Some(2400));
    assert_eq!(cpu.max_freq_mhz(), Some(3100));
    assert!(snap.memory.is_none());
}

#[test]
fn cpu_without_cpufreq_reports_unknown_frequency() {
    let root = fixture("no-cpufreq");
    let mut collector = CpuCollector::new(&root).unwrap();
    let mut snap = Snapshot::new();
    collector.collect(&root, &mut snap).unwrap();
    let cpu = snap.cpu.unwrap();
    assert!(cpu.cores.iter().all(|c| c.freq_mhz.is_none()));
    assert_eq!(cpu.max_freq_mhz(), None);
}

#[test]
fn missing_root_disables_module_instead_of_panicking() {
    let root = SysRoot::at("/nonexistent/corewatch-test");
    assert!(CpuCollector::new(&root).is_err());

    let mut snap = Snapshot::new();
    assert!(MemoryCollector.collect(&root, &mut snap).is_err());
    assert!(snap.memory.is_none());
}

#[test]
fn one_failing_collector_does_not_stop_the_others() {
    // Memory reads a missing root and fails; CPU reads a valid one.
    struct Broken;
    impl Collector for Broken {
        fn name(&self) -> &'static str {
            "broken"
        }
        fn collect(
            &mut self,
            _: &SysRoot,
            _: &mut Snapshot,
        ) -> Result<(), corewatch_core::CollectError> {
            Err(corewatch_core::CollectError::Parse {
                what: "test",
                detail: "always fails".into(),
            })
        }
    }

    let root = fixture("basic");
    let mut sampler = Sampler::new(root.clone(), Duration::from_secs(1));
    sampler.add(Box::new(Broken));
    sampler.add(Box::new(CpuCollector::new(&root).unwrap()));
    sampler.add(Box::new(MemoryCollector));

    let snap = sampler.sample_once();
    assert!(snap.cpu.is_some());
    assert!(snap.memory.is_some());
}

#[test]
fn sampler_thread_delivers_and_stops_on_drop() {
    let root = fixture("basic");
    let mut sampler = Sampler::new(root, Duration::from_millis(20));
    sampler.add(Box::new(MemoryCollector));

    let (tx, rx) = async_channel::bounded(1);
    let handle = sampler.spawn(tx).unwrap();

    let snap = rx.recv_blocking().unwrap();
    assert!(snap.memory.is_some());

    // Dropping the handle must return quickly and close the channel.
    drop(handle);
    // Drain at most one buffered snapshot; after that the channel must be closed.
    let _ = rx.try_recv();
    assert!(rx.recv_blocking().is_err());
}
