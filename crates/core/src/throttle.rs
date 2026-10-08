//! Keeps a collector that fails every tick from flooding the log.
//!
//! The first failure is reported, repeats are counted silently and summarized
//! once per [`SUMMARY_EVERY`], and recovery is reported with the count. Pure
//! state machine: no clock or thread inside, so it is tested directly.

use std::time::{Duration, Instant};

/// How often a still-failing collector is summarized.
pub const SUMMARY_EVERY: Duration = Duration::from_secs(60);

/// What to log for one tick, if anything.
#[derive(Debug, PartialEq, Eq)]
pub enum Report {
    /// First failure after a healthy period: log at `warn` with the full error.
    First,
    /// Still failing; `count` failures since the first. Log at `warn`.
    Ongoing { count: u64 },
    /// Healthy again after `count` failed ticks: log at `info`.
    Recovered { count: u64 },
}

#[derive(Debug, Default)]
pub struct FailureTracker {
    count: u64,
    last_report: Option<Instant>,
}

impl FailureTracker {
    /// Feed one tick's outcome; returns what (if anything) to log.
    pub fn record(&mut self, ok: bool, now: Instant) -> Option<Report> {
        if ok {
            let count = std::mem::take(&mut self.count);
            self.last_report = None;
            return (count > 0).then_some(Report::Recovered { count });
        }

        self.count += 1;
        match self.last_report {
            None => {
                self.last_report = Some(now);
                Some(Report::First)
            }
            Some(at) if now.saturating_duration_since(at) >= SUMMARY_EVERY => {
                self.last_report = Some(now);
                Some(Report::Ongoing { count: self.count })
            }
            Some(_) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn healthy_ticks_report_nothing() {
        let mut t = FailureTracker::default();
        assert_eq!(t.record(true, Instant::now()), None);
    }

    #[test]
    fn first_failure_reports_then_stays_quiet_then_summarizes() {
        let t0 = Instant::now();
        let mut t = FailureTracker::default();
        assert_eq!(t.record(false, t0), Some(Report::First));
        assert_eq!(t.record(false, t0 + Duration::from_secs(1)), None);
        assert_eq!(
            t.record(false, t0 + SUMMARY_EVERY),
            Some(Report::Ongoing { count: 3 })
        );
        assert_eq!(t.record(false, t0 + SUMMARY_EVERY), None);
    }

    #[test]
    fn recovery_reports_count_and_resets() {
        let t0 = Instant::now();
        let mut t = FailureTracker::default();
        t.record(false, t0);
        t.record(false, t0);
        assert_eq!(t.record(true, t0), Some(Report::Recovered { count: 2 }));
        assert_eq!(t.record(true, t0), None);
        assert_eq!(t.record(false, t0), Some(Report::First));
    }
}
