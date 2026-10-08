//! Modules: one collector (data, in `corewatch-core`) plus one page (UI, here).
//!
//! To add a resource, create `modules/<name>.rs` with a page and a `MODULE`
//! constant, then add it to [`all`]. Nothing else in the app changes.

pub mod cpu;
pub mod disk;
pub mod memory;

use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};

/// The UI half of a module. Lives on the GTK main thread.
///
/// Rules every page follows:
/// - read only your own field of [`Snapshot`];
/// - keep all widgets inside the page; share drawing through `crate::widgets`;
/// - format numbers here, never in `corewatch-core`.
pub trait ResourcePage {
    /// Stable id, used as the stack page name, e.g. `"cpu"`.
    fn id(&self) -> &'static str;

    /// Translated name shown in the sidebar and the header bar.
    fn title(&self) -> String;

    fn icon_name(&self) -> &'static str;

    fn widget(&self) -> gtk::Widget;

    /// Called once per snapshot, also while the page is hidden, so graphs keep
    /// their history.
    fn update(&self, snapshot: &Snapshot);

    /// Short live text for the sidebar row, e.g. `"37%"`.
    fn summary(&self, snapshot: &Snapshot) -> Option<String>;
}

/// A module = one collector + one page.
pub struct Module {
    /// May fail (missing file, missing driver). The module is then skipped and
    /// the app still starts.
    pub collector: fn(&SysRoot) -> Result<Box<dyn Collector>, CollectError>,
    pub page: fn() -> Box<dyn ResourcePage>,
}

/// The single registry. Order here is the order in the sidebar.
pub fn all() -> Vec<Module> {
    vec![cpu::MODULE, memory::MODULE, disk::MODULE]
}
