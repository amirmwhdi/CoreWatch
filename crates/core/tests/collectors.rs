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

#[test]
fn sampler_set_interval_adjusts_speed() {
    let root = fixture("basic");
    let mut sampler = Sampler::new(root, Duration::from_secs(10));
    sampler.add(Box::new(MemoryCollector));

    let (tx, rx) = async_channel::bounded(1);
    let handle = sampler.spawn(tx).unwrap();

    let _ = rx.recv_blocking().unwrap();

    // With a 10s interval, the next snapshot would take 10s.
    // Changing interval to 10ms should wake and deliver very quickly.
    let start = std::time::Instant::now();
    handle.set_interval(Duration::from_millis(10));
    let next = rx.recv_blocking().unwrap();
    assert!(next.memory.is_some());
    assert!(start.elapsed() < Duration::from_secs(2));

    drop(handle);
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
        assert_eq!(nvme.temp_c, Some(41.85));
        assert_eq!(disk.disks[1].kind, DiskKind::Hdd);
        assert_eq!(disk.disks[1].temp_c, Some(33.0));
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

#[test]
fn battery_collector_skips_peripherals_and_reads_charge_units() {
    use corewatch_core::battery::{BatteryCollector, BatteryState, REFRESH};
    use std::time::Instant;

    let root = fixture("laptop");
    let mut collector = BatteryCollector::new(&root).unwrap();
    let start = Instant::now();
    let mut snap = Snapshot::new();
    collector.collect_at(&mut snap, start);

    let sample = snap.battery.unwrap();
    assert_eq!(sample.ac_online, Some(false));
    // The wireless mouse (scope = Device) is not listed.
    assert_eq!(sample.batteries.len(), 1);
    let bat = &sample.batteries[0];
    assert_eq!(bat.name, "BAT0");
    assert_eq!(bat.model, "Fixture Cell");
    assert_eq!(bat.state, BatteryState::Discharging);
    assert_eq!(bat.capacity, 0.8);
    // 3 Ah at 12 V = 36 Wh; a negative current still means 12 W.
    assert_eq!(bat.energy_wh, Some(36.0));
    assert_eq!(bat.power_w, Some(12.0));
    assert_eq!(bat.time_remaining_secs, Some(3 * 3600));
    assert_eq!(bat.health, Some(0.9375));

    // Within REFRESH the last reading is reused.
    let mut again = Snapshot::new();
    collector.collect_at(&mut again, start + REFRESH / 2);
    assert_eq!(again.battery.unwrap(), sample);
}

#[test]
fn missing_power_supply_disables_battery() {
    use corewatch_core::battery::BatteryCollector;
    assert!(BatteryCollector::new(&fixture("basic")).is_err());
}

#[test]
fn intel_gpu_load_comes_from_its_idle_time() {
    use corewatch_core::gpu::{GpuCollector, GpuVendor};

    let root = fixture("laptop");
    let mut collector = GpuCollector::new(&root).unwrap();
    let mut snap = Snapshot::new();
    collector.collect(&root, &mut snap).unwrap();

    let gpu = snap.gpu.take().unwrap();
    // card0-eDP-1 is a connector, not a second card.
    assert_eq!(gpu.devices.len(), 1);
    let dev = &gpu.devices[0];
    assert_eq!(dev.vendor, GpuVendor::Intel);
    assert_eq!(dev.driver.as_deref(), Some("i915"));
    // The load needs two readings of the RC6 counter.
    assert_eq!(dev.usage, None);
    assert_eq!(dev.freq_mhz, Some(700));
    assert_eq!(dev.freq_fraction(), Some(0.5));
    assert_eq!(dev.temp_c, Some(45));

    // The fixture's counter does not move: the GT was never idle.
    std::thread::sleep(std::time::Duration::from_millis(20));
    collector.collect(&root, &mut snap).unwrap();
    assert_eq!(snap.gpu.unwrap().devices[0].usage, Some(1.0));
}

#[test]
fn gpus_of_every_vendor_report_what_their_driver_has() {
    use corewatch_core::gpu::{GpuCollector, GpuVendor};

    let root = fixture("gpus");
    let mut collector = GpuCollector::new(&root).unwrap();
    let mut snap = Snapshot::new();
    collector.collect(&root, &mut snap).unwrap();
    let gpus = snap.gpu.unwrap().devices;

    // The network card is not a GPU; Intel's integrated one comes last.
    let ids: Vec<_> = gpus.iter().map(|g| g.id.as_str()).collect();
    assert_eq!(ids, ["0000:01:00.0", "0000:02:00.0", "0000:00:02.0"]);

    let amd = &gpus[0];
    assert_eq!(amd.name, "AMD Radeon RX 580 Armor 4G OC");
    assert_eq!(amd.vendor, GpuVendor::Amd);
    assert_eq!(amd.driver.as_deref(), Some("amdgpu"));
    assert_eq!(amd.usage, Some(0.37));
    assert_eq!(amd.vram_total, Some(8 << 30));
    // The measured clock, and the top level as the maximum.
    assert_eq!(amd.freq_mhz, Some(1150));
    assert_eq!(amd.max_freq_mhz, Some(1366));
    assert_eq!(amd.mem_freq_mhz, Some(2000));
    assert_eq!(amd.mem_max_freq_mhz, Some(2000));
    assert_eq!(amd.temp_c, Some(61));
    assert_eq!(amd.power_w, Some(98.0));
    assert_eq!(amd.power_cap_w, Some(145.0));
    assert_eq!(amd.fan_rpm, Some(1210));

    // Asleep: named, but nothing read (reading would wake it).
    let nvidia = &gpus[1];
    assert_eq!(nvidia.name, "NVIDIA GeForce RTX 3060 Lite Hash Rate");
    assert!(nvidia.asleep);
    assert_eq!(nvidia.temp_c, None);

    let intel = &gpus[2];
    assert_eq!(intel.name, "Intel Iris Xe Graphics");
    assert_eq!(intel.driver.as_deref(), Some("xe"));
    assert_eq!(intel.usage, None); // until the second reading
    assert_eq!(
        (intel.freq_mhz, intel.max_freq_mhz),
        (Some(800), Some(1300))
    );
}

#[test]
fn network_collector_lists_physical_interfaces_and_tunnels() {
    use corewatch_core::network::{InterfaceKind, NetworkCollector};
    use std::time::Instant;

    let root = fixture("desktop");
    let t0 = Instant::now();
    let mut collector = NetworkCollector::starting_at(&root, t0).unwrap();
    let mut snap = Snapshot::new();
    collector
        .collect_at(&root, &mut snap, t0 + Duration::from_secs(1))
        .unwrap();
    let net = snap.network.unwrap();
    let names: Vec<&str> = net.interfaces.iter().map(|i| i.name.as_str()).collect();
    // lo and docker0 are hidden; the tunnel comes last.
    assert_eq!(names, ["enp3s0", "wlp2s0", "wg0"]);

    let eth = &net.interfaces[0];
    assert_eq!(eth.kind, InterfaceKind::Ethernet);
    assert!(eth.connected);
    assert_eq!(eth.link_mbps, Some(1000));
    assert_eq!(eth.mac.as_deref(), Some("a4:5e:60:00:00:01"));
    // Same file twice: no traffic in between.
    assert_eq!(eth.rx_bps, Some(0));
    assert_eq!(eth.rx_total, 1_000_000);

    let wifi = &net.interfaces[1];
    assert_eq!(wifi.kind, InterfaceKind::WiFi);
    assert!((wifi.signal.unwrap() - 0.5).abs() < 1e-6);
    assert_eq!(wifi.link_mbps, None);

    let vpn = &net.interfaces[2];
    assert_eq!(vpn.kind, InterfaceKind::Vpn);
    assert!(vpn.connected);
    assert_eq!(vpn.mac, None);
}

#[test]
fn sensor_collector_reads_temperatures_and_fans() {
    use corewatch_core::sensors::{ChipKind, SensorCollector};

    let root = fixture("desktop");
    let mut collector = SensorCollector::new(&root).unwrap();
    let mut snap = Snapshot::new();
    collector.collect(&root, &mut snap).unwrap();
    let sensors = snap.sensors.unwrap();

    let kinds: Vec<ChipKind> = sensors.chips.iter().map(|c| c.kind).collect();
    // acpi_fan has no inputs and is left out.
    assert_eq!(kinds, [ChipKind::Cpu, ChipKind::Storage, ChipKind::Board]);
    assert_eq!(sensors.cpu_celsius(), Some(61.25));
    assert_eq!(sensors.hottest_celsius(), Some(61.25));

    let nvme = &sensors.chips[1];
    assert_eq!(nvme.model.as_deref(), Some("Samsung SSD 980 PRO 1TB"));
    assert_eq!(nvme.temperatures[0].critical, Some(84.85));

    let board = &sensors.chips[2];
    assert_eq!(board.fans.len(), 2);
    assert_eq!(board.fans[0].label.as_deref(), Some("CPU_FAN"));
    assert_eq!(board.fans[0].rpm, 1180);
}

#[test]
fn process_collector_lists_processes_with_full_names() {
    use corewatch_core::process::{ProcessCollector, ProcessSource};
    use std::sync::atomic::AtomicBool;

    static ACTIVE: AtomicBool = AtomicBool::new(true);
    let root = fixture("desktop");
    let mut collector = ProcessCollector::new(&root, ProcessSource::Direct, &ACTIVE).unwrap();
    let mut snap = Snapshot::new();
    collector.collect(&root, &mut snap).unwrap();
    let sample = snap.processes.unwrap();

    let mut list = sample.processes.to_vec();
    list.sort_by_key(|p| p.pid);
    assert_eq!(list.len(), 2);
    assert_eq!(&*list[0].name, "systemd");
    // The kernel's name is cut at 15 bytes; the command line completes it.
    assert_eq!(&*list[1].name, "gnome-shell-calendar-server");
    assert_eq!(list[1].threads, 4);
    assert_eq!(list[1].state, 'R');
    let page = u64::try_from(rustix::param::page_size()).unwrap();
    assert_eq!(list[1].memory, 2000 * page);
    assert!(!list[1].user.is_empty());
    // Nothing ran between the two scans of the same files.
    assert_eq!(list[1].cpu, 0.0);
    assert_eq!(sample.generation, 2);
}

#[test]
fn system_collector_reads_uptime_and_load() {
    use corewatch_core::system::SystemCollector;

    let mut snap = Snapshot::new();
    SystemCollector
        .collect(&fixture("desktop"), &mut snap)
        .unwrap();
    let system = snap.system.unwrap();
    assert_eq!(system.uptime_secs, 93784);
    assert_eq!(system.load, [0.52, 0.58, 0.59]);
}
