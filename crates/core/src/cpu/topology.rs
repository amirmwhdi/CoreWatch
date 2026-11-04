//! Performance and efficient cores, on processors that have both.
//!
//! Intel's hybrid processors (12th generation and later) list their cores
//! by type in `/sys/devices/cpu_core/cpus` (performance) and
//! `/sys/devices/cpu_atom/cpus` (efficient). On ARM big.LITTLE chips each
//! core has a `cpu_capacity`: the largest is a performance core, smaller
//! ones are efficient. Everything else has cores that are all alike.

use crate::SysRoot;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreKind {
    /// P-core, or a "big" core.
    Performance,
    /// E-core, or a "LITTLE" core.
    Efficient,
}

/// Each logical CPU's kind, and how many physical cores of each kind there
/// are. Empty when all cores are alike.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Topology {
    kinds: HashMap<usize, CoreKind>,
    /// Physical cores (not threads) of each kind.
    pub performance_cores: usize,
    pub efficient_cores: usize,
}

impl Topology {
    /// Read once at start; `ids` are the logical CPUs `/proc/stat` lists.
    pub fn read(root: &SysRoot, ids: &[usize]) -> Self {
        let kinds = intel_hybrid(root).or_else(|| arm_capacity(root, ids));
        let Some(kinds) = kinds.filter(|k| {
            let found: HashSet<_> = k.values().collect();
            found.len() == 2
        }) else {
            return Self::default();
        };
        let cores = |kind: CoreKind| {
            kinds
                .iter()
                .filter(|(_, k)| **k == kind)
                .map(|(cpu, _)| physical_core(root, *cpu))
                .collect::<HashSet<_>>()
                .len()
        };
        Self {
            performance_cores: cores(CoreKind::Performance),
            efficient_cores: cores(CoreKind::Efficient),
            kinds,
        }
    }

    /// The kind of logical CPU `id`; `None` when cores are all alike.
    pub fn kind(&self, id: usize) -> Option<CoreKind> {
        self.kinds.get(&id).copied()
    }

    pub fn is_hybrid(&self) -> bool {
        !self.kinds.is_empty()
    }
}

fn intel_hybrid(root: &SysRoot) -> Option<HashMap<usize, CoreKind>> {
    let list = |dir: &str| {
        root.read_sys(&format!("devices/{dir}/cpus"))
            .ok()
            .and_then(|text| parse_cpu_list(&text))
    };
    let (p, e) = (list("cpu_core")?, list("cpu_atom")?);
    let mut kinds: HashMap<usize, CoreKind> = p
        .into_iter()
        .map(|cpu| (cpu, CoreKind::Performance))
        .collect();
    kinds.extend(e.into_iter().map(|cpu| (cpu, CoreKind::Efficient)));
    Some(kinds)
}

fn arm_capacity(root: &SysRoot, ids: &[usize]) -> Option<HashMap<usize, CoreKind>> {
    let capacities: Vec<(usize, u32)> = ids
        .iter()
        .filter_map(|&cpu| {
            let text = root
                .read_sys(&format!("devices/system/cpu/cpu{cpu}/cpu_capacity"))
                .ok()?;
            Some((cpu, text.trim().parse().ok()?))
        })
        .collect();
    let top = capacities.iter().map(|(_, c)| *c).max()?;
    Some(
        capacities
            .into_iter()
            .map(|(cpu, capacity)| {
                let kind = if capacity == top {
                    CoreKind::Performance
                } else {
                    CoreKind::Efficient
                };
                (cpu, kind)
            })
            .collect(),
    )
}

/// Which physical core a logical CPU belongs to: (package, core id), or the
/// CPU itself when the kernel does not say.
fn physical_core(root: &SysRoot, cpu: usize) -> (i64, i64) {
    let read = |name: &str| {
        root.read_sys(&format!("devices/system/cpu/cpu{cpu}/topology/{name}"))
            .ok()
            .and_then(|t| t.trim().parse::<i64>().ok())
    };
    match (read("physical_package_id"), read("core_id")) {
        (Some(package), Some(core)) => (package, core),
        _ => (-1, cpu as i64),
    }
}

/// Parse a kernel CPU list: `"0-3,8,10-11"`.
pub fn parse_cpu_list(text: &str) -> Option<Vec<usize>> {
    let mut cpus = Vec::new();
    for part in text.trim().split(',').filter(|p| !p.is_empty()) {
        match part.split_once('-') {
            Some((from, to)) => {
                let (from, to): (usize, usize) = (from.parse().ok()?, to.parse().ok()?);
                if to < from || to - from > 65536 {
                    return None;
                }
                cpus.extend(from..=to);
            }
            None => cpus.push(part.parse().ok()?),
        }
    }
    Some(cpus)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cpu_lists() {
        assert_eq!(
            parse_cpu_list("0-3,8,10-11\n"),
            Some(vec![0, 1, 2, 3, 8, 10, 11])
        );
        assert_eq!(parse_cpu_list("5"), Some(vec![5]));
        assert_eq!(parse_cpu_list(""), Some(vec![]));
        assert_eq!(parse_cpu_list("3-1"), None);
        assert_eq!(parse_cpu_list("a"), None);
    }
}
