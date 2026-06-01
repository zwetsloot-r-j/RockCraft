//! RockCraft terminal frontend (MVP).
//!
//! The swappable view layer — the same `core` engine will later drive a Tauri
//! desktop app and a Godot visual frontend. This M0 build connects to the piano
//! and shows a bottom keyboard that lights up held keys, with a live event log
//! above it.
//!
//! Run on Windows (where the CASIO USB-MIDI device is visible):
//!
//! ```text
//! cargo run -p rockcraft-tui
//! ```
//!
//! Optional argument: a substring of the MIDI port name (default "casio").

mod app;
mod keyboard;

use rockcraft_midi::LiveInput;

fn main() {
    let filter = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "casio".to_string());

    let input = match LiveInput::connect(&filter) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("Could not open MIDI input: {e}");
            eprintln!("(Is the piano connected? Try passing a port-name substring.)");
            std::process::exit(1);
        }
    };

    if let Err(e) = app::run(input) {
        eprintln!("TUI error: {e}");
        std::process::exit(1);
    }
}
