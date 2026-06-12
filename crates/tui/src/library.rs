//! Re-exports the bundle scanner from `rockcraft_midi::bundle`.
//!
//! The scanner was moved to `crates/midi` so both the TUI and Tauri backends
//! share one implementation. All call sites in this crate continue to compile
//! without changes.
pub use rockcraft_midi::bundle::*;
