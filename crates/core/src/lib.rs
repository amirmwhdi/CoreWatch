//! Data collection for Corewatch.
//!
//! This crate turns kernel files (`/proc`, `/sys`) into typed numbers. It knows
//! nothing about GTK, formatting or translation: fractions are 0.0..=1.0 and
//! sizes are bytes. The app decides how to show them.
//!
//! Every resource is one [`Collector`] that fills one field of [`Snapshot`].
//! The [`Sampler`] runs all collectors on a background thread.

#![cfg_attr(
    not(test),
    deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)
)]

pub mod collector;
pub mod cpu;
pub mod error;
pub mod memory;
pub mod root;
pub mod sampler;
pub mod snapshot;

pub use collector::Collector;
pub use error::CollectError;
pub use root::SysRoot;
pub use sampler::{Sampler, SamplerHandle};
pub use snapshot::Snapshot;
