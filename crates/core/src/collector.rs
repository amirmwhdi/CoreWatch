use crate::{CollectError, Snapshot, SysRoot};

/// The data half of a module. Runs on the sampler thread.
///
/// Rules every collector follows:
/// - never panic and never block (no sleeping, no network);
/// - write only your own field of [`Snapshot`];
/// - keep state between calls (previous counters) inside `self`;
/// - output raw units: fractions 0.0..=1.0, bytes, MHz.
pub trait Collector: Send {
    /// Short stable name for logs, e.g. `"cpu"`.
    fn name(&self) -> &'static str;

    /// Read the current state and write it into `out`.
    fn collect(&mut self, root: &SysRoot, out: &mut Snapshot) -> Result<(), CollectError>;
}
