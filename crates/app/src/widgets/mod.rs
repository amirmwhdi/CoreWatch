//! Widgets shared by every module page. Page layouts live in Blueprint
//! (`data/ui/stable/`); margins in `data/style.css`.

pub mod graph;
pub mod page_frame;
pub mod sensor_group;

pub use graph::Graph;
pub use page_frame::PageFrame;
pub use sensor_group::SensorGroup;
