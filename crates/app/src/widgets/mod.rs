//! Widgets shared by every module page. Page layouts live in Blueprint
//! (`data/ui/stable/`); margins in `data/style.css`.

pub mod graph;
pub mod page_frame;

pub use graph::Graph;
pub use page_frame::PageFrame;
