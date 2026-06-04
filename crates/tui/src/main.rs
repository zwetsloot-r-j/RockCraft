//! RockCraft terminal frontend entry point.
//!
//! Thin binary wrapper — the app logic lives in `lib.rs` (the library target
//! of this crate) so integration tests in `tests/` can access it.

use rockcraft_audio::AudioOut;
use rockcraft_midi::{LiveInput, MockKeyboard, NoteSource};
use rockcraft_tui::app;

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
    // Optional second arg: path to a backing audio file (wav/mp3/ogg/flac).
    // When provided, entering Record plays it alongside your performance.
    let backing_path = std::env::args().nth(2).map(std::path::PathBuf::from);

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

    if let Err(e) = app::run(input, synth, backing_path) {
        eprintln!("TUI error: {e}");
        std::process::exit(1);
    }
}
