//! The background thread that runs every collector once per tick.
//!
//! The sampler does not know which modules exist: it only holds
//! `Box<dyn Collector>` and sends finished [`Snapshot`]s over a channel.

use crate::throttle::{FailureTracker, Report};
use crate::{Collector, Snapshot, SysRoot};
use std::sync::mpsc::{self, RecvTimeoutError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// One collector plus its failure history, so a permanent failure is logged
/// once and then summarized instead of once per tick.
struct Slot {
    collector: Box<dyn Collector>,
    failures: FailureTracker,
}

pub struct Sampler {
    root: SysRoot,
    interval: Duration,
    slots: Vec<Slot>,
}

impl Sampler {
    pub fn new(root: SysRoot, interval: Duration) -> Self {
        Self {
            root,
            interval,
            slots: Vec::new(),
        }
    }

    pub fn add(&mut self, collector: Box<dyn Collector>) {
        self.slots.push(Slot {
            collector,
            failures: FailureTracker::default(),
        });
    }

    /// Number of registered collectors.
    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    /// Run one tick on the calling thread. Used by tests and the example.
    pub fn sample_once(&mut self) -> Snapshot {
        let mut snapshot = Snapshot::new();
        for slot in &mut self.slots {
            let name = slot.collector.name();
            let started = Instant::now();
            let result = slot.collector.collect(&self.root, &mut snapshot);
            let elapsed_us = u64::try_from(started.elapsed().as_micros()).unwrap_or(u64::MAX);
            tracing::debug!(
                collector = name,
                elapsed_us,
                ok = result.is_ok(),
                "collected"
            );

            match (
                slot.failures.record(result.is_ok(), Instant::now()),
                &result,
            ) {
                (Some(Report::First), Err(error)) => tracing::warn!(
                    collector = name,
                    error.kind = error.kind(),
                    error = %error.chain(),
                    "collector failed"
                ),
                (Some(Report::Ongoing { count }), Err(error)) => tracing::warn!(
                    collector = name,
                    error.kind = error.kind(),
                    count,
                    "collector still failing"
                ),
                (Some(Report::Recovered { count }), _) => tracing::info!(
                    collector = name,
                    failed_ticks = count,
                    "collector recovered"
                ),
                _ => {}
            }
        }
        snapshot
    }

    /// Start the background thread.
    ///
    /// Use a channel with capacity 1: `force_send` replaces an unread snapshot,
    /// so a slow UI always gets the newest one instead of a backlog.
    pub fn spawn(mut self, tx: async_channel::Sender<Snapshot>) -> std::io::Result<SamplerHandle> {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let thread = std::thread::Builder::new()
            .name("corewatch-sampler".into())
            .spawn(move || {
                let mut next = Instant::now();
                loop {
                    if tx.force_send(self.sample_once()).is_err() {
                        break; // receiver dropped: the window is gone
                    }

                    next += self.interval;
                    let now = Instant::now();
                    if next <= now {
                        next = now; // fell behind: skip ticks instead of bursting
                    }
                    // Sleep until the next tick, but wake at once when asked to stop.
                    match stop_rx.recv_timeout(next - now) {
                        Err(RecvTimeoutError::Timeout) => {}
                        Ok(()) | Err(RecvTimeoutError::Disconnected) => break,
                    }
                }
                tracing::debug!("sampler stopped");
            })?;

        Ok(SamplerHandle {
            stop: Some(stop_tx),
            thread: Some(thread),
        })
    }
}

/// Owns the sampler thread. Dropping it stops the thread right away and waits
/// for it to finish.
pub struct SamplerHandle {
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for SamplerHandle {
    fn drop(&mut self) {
        drop(self.stop.take()); // disconnecting wakes the sleeping thread
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
