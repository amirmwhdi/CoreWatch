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

// ---- Disk ------------------------------------------------------------------

mod disk {
    use super::fixture;
    use corewatch_core::disk::{DiskCollector, DiskKind, Space, SpaceProbe};
    use corewatch_core::{Collector, Snapshot, SysRoot};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    /// Space numbers for a fixed set of mount points; counts how often it is asked.
    struct FakeProbe {
        known: Vec<(PathBuf, Space)>,
        calls: Arc<AtomicUsize>,
    }

    impl SpaceProbe for FakeProbe {
        fn space(&self, mount_point: &Path) -> Option<Space> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            self.known
                .iter()
                .find(|(p, _)| p == mount_point)
                .map(|(_, s)| *s)
        }
    }

    fn probe(points: &[&str]) -> (Box<dyn SpaceProbe>, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let known = points
            .iter()
            .map(|p| (PathBuf::from(p), Space::from_blocks(1000, 250, 200, 4096)))
            .collect();
        let probe = FakeProbe {
            known,
            calls: Arc::clone(&calls),
        };
        (Box::new(probe), calls)
    }

    /// `/proc` from the "disk-later" fixture, `/sys` from "basic".
    fn later() -> SysRoot {
        let base = format!("{}/tests/fixtures", env!("CARGO_MANIFEST_DIR"));
        SysRoot::new(
            format!("{base}/disk-later/proc"),
            format!("{base}/basic/sys"),
        )
    }

    #[test]
    fn lists_physical_disks_only() {
        let root = fixture("basic");
        let t0 = Instant::now();
        let (p, _) = probe(&["/"]);
        let mut collector = DiskCollector::with_probe(&root, p, t0).unwrap();
        let mut snap = Snapshot::new();
        collector
            .collect_at(&root, &mut snap, t0 + Duration::from_secs(1))
            .unwrap();

        let disk = snap.disk.unwrap();
        let names: Vec<_> = disk.disks.iter().map(|d| d.name.as_str()).collect();
        // No partitions, no loop or dm devices, no empty card reader (sdc),
        // and sdb is not in this diskstats.
        assert_eq!(names, ["nvme0n1", "sda"]);

        let nvme = &disk.disks[0];
        assert_eq!(nvme.model, "Fixture NVMe SSD 512GB");
        assert_eq!(nvme.kind, DiskKind::Nvme);
        assert_eq!(nvme.size, 1_000_215_216 * 512);
        assert_eq!(nvme.read_total, 200_000 * 512);
        assert_eq!(nvme.written_total, 400_000 * 512);
        // Same counters as at start: idle, but known.
        assert_eq!(nvme.read_bps, Some(0));
        assert_eq!(nvme.busy, Some(0.0));
        assert_eq!(disk.disks[1].kind, DiskKind::Hdd);
        assert!(snap.cpu.is_none() && snap.memory.is_none());
    }

    #[test]
    fn speeds_and_activity_between_two_samples() {
        let t0 = Instant::now();
        let (p, _) = probe(&[]);
        let mut collector = DiskCollector::with_probe(&fixture("basic"), p, t0).unwrap();
        let mut snap = Snapshot::new();
        collector
            .collect_at(&later(), &mut snap, t0 + Duration::from_secs(2))
            .unwrap();
        let disk = snap.disk.unwrap();

        let nvme = disk.disks.iter().find(|d| d.name == "nvme0n1").unwrap();
        assert_eq!(nvme.read_bps, Some(20_000 * 512 / 2));
        assert_eq!(nvme.write_bps, Some(40_000 * 512 / 2));
        assert_eq!(nvme.busy, Some(0.25));

        // Counters went down: treated as idle, not as a huge jump.
        let sda = disk.disks.iter().find(|d| d.name == "sda").unwrap();
        assert_eq!(sda.read_bps, Some(0));
        assert_eq!(sda.busy, Some(0.0));

        // Plugged in since the last sample: no speed yet.
        let sdb = disk.disks.iter().find(|d| d.name == "sdb").unwrap();
        assert_eq!(sdb.kind, DiskKind::Removable);
        assert_eq!(sdb.model, "Fixture USB");
        assert_eq!(sdb.read_bps, None);
        assert_eq!(sdb.busy, None);

        assert_eq!(disk.total_bps(), (20_000 + 40_000) * 512 / 2);
    }

    #[test]
    fn filesystems_are_local_and_skip_unanswered_mounts() {
        let root = fixture("basic");
        // "/boot/efi" gets no answer from the probe and must be left out.
        let (p, _) = probe(&["/", "/mnt/My Data"]);
        let mut collector = DiskCollector::with_probe(&root, p, Instant::now()).unwrap();
        let mut snap = Snapshot::new();
        collector.collect(&root, &mut snap).unwrap();

        let fs = snap.disk.unwrap().filesystems;
        let points: Vec<_> = fs.iter().map(|f| f.mount_point.clone()).collect();
        assert_eq!(points, [PathBuf::from("/"), PathBuf::from("/mnt/My Data")]);
        assert_eq!(fs[0].fs_type, "ext4");
        assert_eq!(fs[0].source, "/dev/nvme0n1p2");
        assert_eq!(fs[0].space.total, 4_096_000);
    }

    #[test]
    fn space_is_read_only_every_few_ticks() {
        let root = fixture("basic");
        let (p, calls) = probe(&["/"]);
        let mut collector = DiskCollector::with_probe(&root, p, Instant::now()).unwrap();
        let mut snap = Snapshot::new();
        for _ in 0..(corewatch_core::disk::FS_EVERY + 1) {
            collector.collect(&root, &mut snap).unwrap();
        }
        // Three local filesystems, asked on the first tick and on tick FS_EVERY.
        assert_eq!(calls.load(Ordering::Relaxed), 2 * 3);
        // In between, the last list is kept.
        assert_eq!(snap.disk.unwrap().filesystems.len(), 1);
    }

    #[test]
    fn missing_mountinfo_keeps_disks_working() {
        let (p, _) = probe(&["/"]);
        let mut collector = DiskCollector::with_probe(&later(), p, Instant::now()).unwrap();
        let mut snap = Snapshot::new();
        collector.collect(&later(), &mut snap).unwrap();
        let disk = snap.disk.unwrap();
        assert!(!disk.disks.is_empty());
        assert!(disk.filesystems.is_empty());
    }

    #[test]
    fn missing_diskstats_disables_the_module() {
        let error = DiskCollector::new(&SysRoot::at("/nonexistent/corewatch-test"))
            .err()
            .unwrap();
        assert_eq!(error.kind(), "io.not_found");
    }
}
