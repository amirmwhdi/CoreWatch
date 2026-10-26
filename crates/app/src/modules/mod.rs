//! Modules: one collector (data, in `corewatch-core`) plus one page (UI, here).
//!
//! To add a resource, create `modules/<name>.rs` with a page and a `MODULE`
//! constant, then add it to [`all`]. Nothing else in the app changes.

pub mod battery;
pub mod cpu;
pub mod disk;
pub mod gpu;
pub mod memory;
pub mod network;
pub mod overview;
pub mod processes;
pub mod sensors;

use corewatch_core::{CollectError, Collector, Snapshot, SysRoot};

/// The UI half of a module. Lives on the GTK main thread.
///
/// Rules every page follows:
/// - read only your own field of [`Snapshot`];
/// - keep all widgets inside the page; share drawing through `crate::widgets`;
/// - format numbers here, never in `corewatch-core`.
///
/// The overview page is the one exception to the first rule: it shows a
/// little of every field, next to its own (uptime and load).
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

    /// Whether this tick holds the page's data. Three misses in a row show
    /// the "Data unavailable" banner (see `widgets::PageFrame`).
    fn has_data(&self, snapshot: &Snapshot) -> bool;

    /// Short live text for the sidebar row, e.g. `"37%"`.
    fn summary(&self, snapshot: &Snapshot) -> Option<String>;
}

/// Builds a module's collector; see [`Module::collector`].
pub type NewCollector = fn(&SysRoot) -> Result<Box<dyn Collector>, CollectError>;

/// A module = one collector + one page.
pub struct Module {
    /// May fail (missing file, missing driver). The module is then skipped and
    /// the app still starts. `None` for a page that only reads the other
    /// modules' data (the overview).
    pub collector: Option<NewCollector>,
    pub page: fn() -> Box<dyn ResourcePage>,
}

/// The single registry. Order here is the order in the sidebar.
pub fn all() -> Vec<Module> {
    vec![
        overview::MODULE,
        processes::MODULE,
        cpu::MODULE,
        memory::MODULE,
        gpu::MODULE,
        disk::MODULE,
        network::MODULE,
        battery::MODULE,
        sensors::MODULE,
    ]
}
