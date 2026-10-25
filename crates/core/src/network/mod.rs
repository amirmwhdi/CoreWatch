//! Network module: download and upload speed per interface, its addresses,
//! link speed and Wi-Fi signal.

mod addrs;
mod netdev;

pub use addrs::{read_addresses, Addresses};
pub use netdev::{parse_net_dev, parse_wireless, NetCounters, NetDevStat};

use crate::{CollectError, Collector, Snapshot, SysRoot};
use std::collections::HashMap;
use std::time::Instant;

/// Addresses change rarely; read them once every this many ticks.
pub const ADDR_EVERY: u32 = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterfaceKind {
    Ethernet,
    WiFi,
    /// Tunnels: WireGuard, OpenVPN (`tun`/`tap`), PPP.
    Vpn,
    /// Physical, but neither Ethernet nor Wi-Fi (mobile broadband, USB modems).
    Other,
}

/// One network interface in one sample.
#[derive(Debug, Clone, PartialEq)]
pub struct NetInterface {
    /// Kernel name, e.g. `"wlp2s0"`.
    pub name: String,
    pub kind: InterfaceKind,
    /// Hardware address, e.g. `"a4:5e:60:12:34:56"`; `None` for tunnels.
    pub mac: Option<String>,
    /// The link is up and carries traffic.
    pub connected: bool,
    /// Negotiated link speed in Mbit/s; Ethernet only, when connected.
    pub link_mbps: Option<u32>,
    /// Wi-Fi link quality, 0.0..=1.0.
    pub signal: Option<f32>,
    /// Bytes per second since the previous sample; `None` on the first
    /// sample after the interface appeared.
    pub rx_bps: Option<u64>,
    pub tx_bps: Option<u64>,
    /// Bytes received and sent since the interface came up.
    pub rx_total: u64,
    pub tx_total: u64,
    pub addresses: Addresses,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct NetworkSample {
    /// Physical interfaces first, then tunnels; each group sorted by name.
    pub interfaces: Vec<NetInterface>,
}

impl NetworkSample {
    /// Download speed of every interface together, in bytes per second.
    /// Tunnels are left out: their traffic also passes a physical interface.
    pub fn rx_bps(&self) -> u64 {
        self.sum(|i| i.rx_bps)
    }

    /// Upload speed, counted like [`NetworkSample::rx_bps`].
    pub fn tx_bps(&self) -> u64 {
        self.sum(|i| i.tx_bps)
    }

    fn sum(&self, f: impl Fn(&NetInterface) -> Option<u64>) -> u64 {
        self.interfaces
            .iter()
            .filter(|i| i.kind != InterfaceKind::Vpn)
            .map(|i| f(i).unwrap_or(0))
            .fold(0, u64::saturating_add)
    }
}

/// What does not change while an interface exists.
#[derive(Debug, Clone)]
struct StaticInfo {
    /// `None`: an interface Corewatch does not show (bridges, veth, docker).
    kind: Option<InterfaceKind>,
    mac: Option<String>,
}

pub struct NetworkCollector {
    prev: HashMap<String, NetCounters>,
    prev_at: Instant,
    info: HashMap<String, StaticInfo>,
    addresses: HashMap<String, Addresses>,
    /// Ticks left until the next address refresh; 0 = refresh now.
    addr_countdown: u32,
}

impl NetworkCollector {
    /// Reads `net/dev` once to prime the counters, so the first sample already
    /// holds real speeds. Fails only when `net/dev` is unreadable.
    pub fn new(root: &SysRoot) -> Result<Self, CollectError> {
        Self::starting_at(root, Instant::now())
    }

    /// Like [`NetworkCollector::new`] with an explicit start time; used by tests.
    pub fn starting_at(root: &SysRoot, now: Instant) -> Result<Self, CollectError> {
        let prev = parse_net_dev(&root.read_proc("net/dev")?)?
            .into_iter()
            .map(|s| (s.name, s.counters))
            .collect();
        Ok(Self {
            prev,
            prev_at: now,
            info: HashMap::new(),
            addresses: HashMap::new(),
            addr_countdown: 0,
        })
    }

    /// [`Collector::collect`] with an explicit clock; used by tests.
    pub fn collect_at(
        &mut self,
        root: &SysRoot,
        out: &mut Snapshot,
        now: Instant,
    ) -> Result<(), CollectError> {
        let stats = parse_net_dev(&root.read_proc("net/dev")?)?;
        let secs = now.saturating_duration_since(self.prev_at).as_secs_f64();
        let wireless = root
            .read_proc("net/wireless")
            .map(|text| parse_wireless(&text))
            .unwrap_or_default();

        if self.addr_countdown == 0 {
            // Only the real machine has addresses; a fake root has none.
            self.addresses = if root.is_host() {
                read_addresses()
            } else {
                HashMap::new()
            };
            self.addr_countdown = ADDR_EVERY.saturating_sub(1);
        } else {
            self.addr_countdown -= 1;
        }

        let mut interfaces = Vec::new();
        let mut seen = HashMap::new();
        for NetDevStat { name, counters } in stats {
            let info = self
                .info
                .entry(name.clone())
                .or_insert_with(|| read_static(root, &name));
            if let Some(kind) = info.kind {
                let per_sec = |now: u64, before: u64| {
                    // A counter that went down (the interface was recreated)
                    // counts as no traffic for this tick.
                    (now.saturating_sub(before) as f64 / secs).round() as u64
                };
                let rates = self
                    .prev
                    .get(&name)
                    .filter(|_| secs > 0.0)
                    .map(|b| (per_sec(counters.rx, b.rx), per_sec(counters.tx, b.tx)));
                let connected = is_connected(root, &name);
                interfaces.push(NetInterface {
                    kind,
                    mac: info.mac.clone(),
                    connected,
                    link_mbps: (connected && kind == InterfaceKind::Ethernet)
                        .then(|| read_link_mbps(root, &name))
                        .flatten(),
                    signal: wireless.get(&name).copied(),
                    rx_bps: rates.map(|r| r.0),
                    tx_bps: rates.map(|r| r.1),
                    rx_total: counters.rx,
                    tx_total: counters.tx,
                    addresses: self.addresses.get(&name).cloned().unwrap_or_default(),
                    name: name.clone(),
                });
            }
            seen.insert(name, counters);
        }
        interfaces.sort_by(|a, b| {
            (a.kind == InterfaceKind::Vpn, &a.name).cmp(&(b.kind == InterfaceKind::Vpn, &b.name))
        });

        // Forget removed interfaces, so a new one reusing the name is read afresh.
        self.info.retain(|name, _| seen.contains_key(name));
        self.prev = seen;
        self.prev_at = now;

        out.network = Some(NetworkSample { interfaces });
        Ok(())
    }
}

impl Collector for NetworkCollector {
    fn name(&self) -> &'static str {
        "network"
    }

    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError> {
        self.collect_at(root, out, Instant::now())
    }
}

/// ARP hardware types from `<linux/if_arp.h>`.
const ARPHRD_ETHER: u32 = 1;
const ARPHRD_PPP: u32 = 512;
const ARPHRD_NONE: u32 = 65534;

fn read_static(root: &SysRoot, name: &str) -> StaticInfo {
    let dir = format!("class/net/{name}");
    let arp_type = root
        .read_sys(&format!("{dir}/type"))
        .ok()
        .and_then(|t| t.trim().parse::<u32>().ok());
    let virtual_ = root
        .sys_path(&format!("devices/virtual/net/{name}"))
        .exists();
    let tunnel_name = ["tun", "tap", "wg", "ppp", "vpn"]
        .iter()
        .any(|prefix| name.starts_with(prefix));

    let kind = if name == "lo" {
        None
    } else if !virtual_ {
        Some(
            if root.sys_path(&format!("{dir}/wireless")).exists()
                || root.sys_path(&format!("{dir}/phy80211")).exists()
            {
                InterfaceKind::WiFi
            } else if arp_type == Some(ARPHRD_ETHER) {
                InterfaceKind::Ethernet
            } else {
                InterfaceKind::Other
            },
        )
    } else if tunnel_name || matches!(arp_type, Some(ARPHRD_NONE | ARPHRD_PPP)) {
        Some(InterfaceKind::Vpn)
    } else {
        None // bridges, veth pairs, docker0, dummy
    };

    let mac = root
        .read_sys(&format!("{dir}/address"))
        .ok()
        .map(|a| a.trim().to_owned())
        .filter(|a| !a.is_empty() && a != "00:00:00:00:00:00" && kind != Some(InterfaceKind::Vpn));
    StaticInfo { kind, mac }
}

/// `operstate` is `up` for a working link and `unknown` for most tunnels.
fn is_connected(root: &SysRoot, name: &str) -> bool {
    root.read_sys(&format!("class/net/{name}/operstate"))
        .is_ok_and(|s| matches!(s.trim(), "up" | "unknown"))
}

/// `speed` reads -1 or fails (EINVAL) when the driver does not know it.
fn read_link_mbps(root: &SysRoot, name: &str) -> Option<u32> {
    root.read_sys(&format!("class/net/{name}/speed"))
        .ok()?
        .trim()
        .parse::<i64>()
        .ok()
        .and_then(|v| u32::try_from(v).ok())
        .filter(|v| *v > 0)
}
