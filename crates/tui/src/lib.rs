//! RockCraft TUI — public surface exposed for integration testing.
//!
//! The binary (`main.rs`) calls into this library. Integration tests in
//! `tests/` access screens and the shell through this public surface.

pub mod app;
pub mod highway;
pub mod key_source;
pub mod keyboard;
pub mod play;
pub mod record;
pub mod render;
