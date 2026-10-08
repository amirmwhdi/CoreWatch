//! The only place for code that depends on the GTK or libadwaita version.
//!
//! Corewatch builds against a floor (GTK 4.18, libadwaita 1.7, as in Debian 13)
//! and adds newer APIs behind cumulative Cargo features (`adw-1-8`, ...).
//! Feature code calls neutral functions defined here; only this module uses
//! `#[cfg(feature = ...)]` for library versions.
//!
//! 0.1 builds at the floor only, so there is nothing here yet.
