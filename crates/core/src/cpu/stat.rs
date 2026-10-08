//! `/proc/stat` parsing and usage math.
//!
//! `/proc/stat` holds cumulative time counters, not usage. Usage is the busy
//! share of the difference between two reads.

use crate::CollectError;

const WHAT: &str = "/proc/stat";

/// Cumulative busy and idle time of one CPU (or all of them), in kernel ticks.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CpuTimes {
    pub busy: u64,
    pub idle: u64,
}

impl CpuTimes {
    pub fn total(&self) -> u64 {
        self.busy + self.idle
    }

    /// Busy share between `prev` and `self`, in 0.0..=1.0.
    pub fn usage_since(&self, prev: &CpuTimes) -> f32 {
        let total = self.total().saturating_sub(prev.total());
        if total == 0 {
            return 0.0;
        }
        let busy = self.busy.saturating_sub(prev.busy);
        (busy as f64 / total as f64).clamp(0.0, 1.0) as f32
    }
}

/// Counters of one logical core, with the kernel's core number.
///
/// The number comes from the `cpuN` label, so offline cores leave a gap
/// instead of shifting every core after them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoreTimes {
    pub id: usize,
    pub times: CpuTimes,
}

/// Parsed `/proc/stat`: the aggregate line and one entry per online core.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatTimes {
    pub total: CpuTimes,
    pub cores: Vec<CoreTimes>,
}

/// Parse `/proc/stat`.
///
/// Columns: user nice system idle iowait irq softirq steal [guest guest_nice].
/// Guest time is already included in user time, so it is ignored.
pub fn parse_stat(text: &str) -> Result<StatTimes, CollectError> {
    let mut total = None;
    let mut cores = Vec::new();

    for line in text.lines() {
        let mut fields = line.split_ascii_whitespace();
        let Some(label) = fields.next() else { continue };
        let Some(suffix) = label.strip_prefix("cpu") else {
            continue;
        };

        let values = fields
            .take(8)
            .map(str::parse::<u64>)
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| CollectError::parse(WHAT, format!("{label}: {e}")))?;
        if values.len() < 4 {
            return Err(CollectError::parse(
                WHAT,
                format!("{label}: too few columns"),
            ));
        }
        let at = |i: usize| values.get(i).copied().unwrap_or(0);
        let times = CpuTimes {
            busy: at(0) + at(1) + at(2) + at(5) + at(6) + at(7),
            idle: at(3) + at(4),
        };

        if suffix.is_empty() {
            total = Some(times);
        } else {
            let id = suffix
                .parse()
                .map_err(|_| CollectError::parse(WHAT, format!("bad core label {label}")))?;
            cores.push(CoreTimes { id, times });
        }
    }

    let total = total.ok_or_else(|| CollectError::parse(WHAT, "no aggregate cpu line"))?;
    Ok(StatTimes { total, cores })
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: &str = "cpu  100 0 100 800 0 0 0 0 0 0\n\
                     cpu0 50 0 50 400 0 0 0 0 0 0\n\
                     cpu1 50 0 50 400 0 0 0 0 0 0\n\
                     intr 1\nctxt 5\n";
    const B: &str = "cpu  200 0 200 1000 0 0 0 0 0 0\n\
                     cpu0 150 0 150 400 0 0 0 0 0 0\n\
                     cpu1 50 0 50 600 0 0 0 0 0 0\n\
                     intr 2\nctxt 9\n";

    #[test]
    fn usage_between_two_reads() {
        let a = parse_stat(A).unwrap();
        let b = parse_stat(B).unwrap();
        assert_eq!(b.total.usage_since(&a.total), 0.5);
        assert_eq!(b.cores[0].times.usage_since(&a.cores[0].times), 1.0);
        assert_eq!(b.cores[1].times.usage_since(&a.cores[1].times), 0.0);
    }

    #[test]
    fn no_time_passed_is_zero() {
        let a = parse_stat(A).unwrap();
        assert_eq!(a.total.usage_since(&a.total), 0.0);
    }

    #[test]
    fn counters_going_backwards_do_not_underflow() {
        let a = parse_stat(A).unwrap();
        let b = parse_stat(B).unwrap();
        assert_eq!(a.total.usage_since(&b.total), 0.0);
    }

    #[test]
    fn iowait_and_steal_are_counted_correctly() {
        // user nice system idle iowait irq softirq steal
        let s = parse_stat("cpu  1 2 3 4 5 6 7 8 9 10\n").unwrap();
        assert_eq!(s.total.busy, 1 + 2 + 3 + 6 + 7 + 8);
        assert_eq!(s.total.idle, 4 + 5);
    }

    #[test]
    fn core_ids_come_from_labels() {
        let s = parse_stat("cpu  1 0 1 8\ncpu0 1 0 1 8\ncpu3 1 0 1 8\n").unwrap();
        let ids: Vec<_> = s.cores.iter().map(|c| c.id).collect();
        assert_eq!(ids, [0, 3]);
    }

    #[test]
    fn missing_aggregate_line_is_an_error() {
        assert!(parse_stat("cpu0 1 0 1 8\n").is_err());
    }

    #[test]
    fn short_line_is_an_error() {
        assert!(parse_stat("cpu  1 2\n").is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(parse_stat("cpu  a b c d\n").is_err());
    }
}
