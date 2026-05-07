//! Fellowship library crate.
//!
//! Both `fellowship` (TUI) and `fellowship-ctl` (helper CLI) are binaries that
//! depend on this crate. Anything two binaries need to share lives here so the
//! shared types stay single-source-of-truth (notably the JSON shapes in
//! [`crate::runtime`] and the `Surface` keying in [`crate::surface`]).

pub mod agents;
pub mod app;
pub mod beads;
pub mod config;
pub mod debug_log;
pub mod event;
pub mod gh;
pub mod git;
pub mod keymap;
pub mod layout;
pub mod panes;
pub mod runtime;
pub mod surface;
pub mod ui;
