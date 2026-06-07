//! Capture a PNG screenshot of the RockCraft TUI (headless, M4-H).
//!
//! Uses ratatui's `TestBackend` — no real terminal, no MIDI device, no audio —
//! so this runs cleanly in CI and cloud sandboxes.
//!
//! # Usage
//!
//! ```sh
//! # Capture the default menu view at 80×24
//! cargo run --example screenshot --features screenshot
//!
//! # Custom terminal size
//! cargo run --example screenshot --features screenshot -- --width 120 --height 36
//! ```
//!
//! # Outputs
//!
//! | File             | Description                                              |
//! |------------------|----------------------------------------------------------|
//! | `screenshot.png` | Pixel image (`cols × CELL_W` × `rows × CELL_H` px)     |
//! | `render.txt`     | Text dump for diffing / automated assertions             |
//!
//! # Determinism notes
//!
//! - Terminal size is fixed by the `--width`/`--height` flags (defaults 80×24).
//! - The Shell is headless: no clock, no animation, no MIDI.  The rendered
//!   frame is always the static initial state of the menu screen.
//! - For automated assertions prefer `render.txt`; use `screenshot.png` for
//!   human/agent eyeballing of layout and colour.
//!
//! # Action scripting
//!
//! To capture a non-menu screen (editor, play, etc.) or replay an action
//! sequence, connect to the `--control` WebSocket while the app is running
//! and use `{"type":"query","what":"Render"}` (M4-G) to get the text dump
//! programmatically.  PNG capture of an already-running app is a natural
//! next step when a real PTY driver is wired in.

use rockcraft_midi::ScriptedSource;
use rockcraft_tui::{app::Shell, screenshot::render_to_png};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let width: u16 = args
        .windows(2)
        .find(|w| w[0] == "--width")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(80);

    let height: u16 = args
        .windows(2)
        .find(|w| w[0] == "--height")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(24);

    // Headless shell: empty scripted note source, no audio.
    let shell = Shell::new(Box::new(ScriptedSource::new(vec![])), None, None);

    let (png_bytes, text) = render_to_png(&shell, width, height);

    std::fs::write("screenshot.png", &png_bytes).expect("write screenshot.png");
    std::fs::write("render.txt", &text).expect("write render.txt");

    eprintln!(
        "screenshot.png written  ({w}×{h} terminal → {pw}×{ph} px, {bytes} bytes)",
        w = width,
        h = height,
        pw = width as u32 * rockcraft_tui::screenshot::CELL_W,
        ph = height as u32 * rockcraft_tui::screenshot::CELL_H,
        bytes = png_bytes.len(),
    );
    eprintln!("render.txt written      ({} lines)", text.lines().count());
}
