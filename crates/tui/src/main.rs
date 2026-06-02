//! RockCraft terminal frontend (MVP).
//!
//! The swappable view layer — the same `core` engine will later drive a Tauri
//! desktop app and a Godot visual frontend. This build connects to the piano
//! and presents a menu to switch between modes:
//!   - **Record**: keyboard + live event log; save the take to a `.mid`.
//!   - **Play**: a falling-note highway from a recorded `.mid`, with your live
//!     keys lit over it (play-along).
//!
//! Run on Windows (where the CASIO USB-MIDI device is visible):
//!
//! ```text
//! cargo run -p rockcraft-tui
//! ```
//!
//! Optional argument: a substring of the MIDI port name (default "casio").

mod app;
mod highway;
mod keyboard;
mod play;
mod record;
mod render;

use rockcraft_audio::AudioOut;
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

    // Audio is optional: if there's no SoundFont / output device, run silently.
    // `audio` is bound for the whole run so the output stream stays open.
    let audio = match AudioOut::new() {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("Audio disabled: {e}");
            None
        }
    };
    let synth = audio.as_ref().map(AudioOut::synth);

    if let Err(e) = app::run(input, synth) {
        eprintln!("TUI error: {e}");
        std::process::exit(1);
    }
}
