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
use rockcraft_midi::{LiveInput, MockKeyboard, NoteSource};

fn main() {
    // Args: `--mock` forces the keyboard mock; the first non-flag arg is the
    // MIDI port-name substring (default "casio").
    let args: Vec<String> = std::env::args().skip(1).collect();
    let force_mock = args.iter().any(|a| a == "--mock");
    let filter = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "casio".to_string());

    // Source selection: explicit `--mock`, otherwise the live piano — and when
    // no port matches, fall back to the mock so the app always launches.
    let input: Box<dyn NoteSource> = if force_mock {
        eprintln!("Using MockKeyboard (--mock): type the home/QWERTY rows to play notes.");
        Box::new(MockKeyboard::new())
    } else {
        match LiveInput::connect(&filter) {
            Ok(i) => Box::new(i),
            Err(e) => {
                eprintln!(
                    "No MIDI input ({e}); falling back to MockKeyboard — type to play notes."
                );
                Box::new(MockKeyboard::new())
            }
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
