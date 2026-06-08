//! RockCraft TUI — public surface exposed for integration testing.
//!
//! The binary (`main.rs`) calls into this library. Integration tests in
//! `tests/` access screens and the shell through this public surface.

pub mod app;
pub mod backing;
pub mod edit;
pub mod highway;
pub mod import_screen;
pub mod key_source;
pub mod keyboard;
pub mod play;
pub mod record;
pub mod render;

#[cfg(feature = "screenshot")]
pub mod screenshot;
