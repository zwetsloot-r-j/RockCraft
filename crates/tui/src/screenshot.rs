//! Feature-gated PNG screenshot capture via ratatui's `TestBackend` (M4-H).
//!
//! Enable with `--features screenshot`.  The normal workspace build/test is
//! never affected; this module only compiles when the feature is explicitly
//! requested.
//!
//! # Usage
//!
//! ```sh
//! # Default menu view at 80×24
//! cargo run --example screenshot --features screenshot
//!
//! # Boot straight into the editor
//! cargo run --example screenshot --features screenshot -- --edit
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
//! # How the PNG is built
//!
//! Each character cell becomes `CELL_W × CELL_H` pixels.  The cell background
//! colour fills the entire block; non-space characters add a centred foreground
//! rectangle so the colour layout of the UI is clearly visible.  Actual glyph
//! shapes are not rendered — the companion `render.txt` is the source of truth
//! for character content; the PNG is for human/agent eyeballing of layout and
//! colour.
//!
//! # Determinism caveats
//!
//! - Fix terminal size by passing explicit `width`/`height` (defaults 80×24).
//! - The `Shell` is static/headless here (no clock, no animation, no MIDI).
//! - For automated assertions prefer `render.txt`; use `screenshot.png` for
//!   visual review only.

use image::{DynamicImage, ImageBuffer, Rgb};
use ratatui::{backend::TestBackend, style::Color, Terminal};

use crate::app::Shell;

/// Pixel width of one character cell.
pub const CELL_W: u32 = 8;
/// Pixel height of one character cell.
pub const CELL_H: u32 = 16;

/// Render `shell` at the given terminal dimensions and produce a PNG image
/// alongside the companion text dump.
///
/// Returns `(png_bytes, text_dump)`.  The text dump is identical to what
/// [`Shell::render_to_string`] produces; it is included so callers can write
/// both outputs from one call without a second render pass.
pub fn render_to_png(shell: &Shell, width: u16, height: u16) -> (Vec<u8>, String) {
    let backend = TestBackend::new(width, height);
    let mut terminal = Terminal::new(backend).expect("TestBackend init");
    terminal.draw(|f| shell.render(f)).expect("draw");

    let buf = terminal.backend().buffer().clone();
    let content = buf.content();

    // ── Text dump ─────────────────────────────────────────────────────────────
    let w = width as usize;
    let text: String = (0..height as usize)
        .map(|row| {
            let row_str: String = content[row * w..(row + 1) * w]
                .iter()
                .map(|cell| cell.symbol())
                .collect();
            row_str.trim_end().to_string()
        })
        .collect::<Vec<_>>()
        .join("\n");

    // ── Image ─────────────────────────────────────────────────────────────────
    let img_w = width as u32 * CELL_W;
    let img_h = height as u32 * CELL_H;
    let mut img: ImageBuffer<Rgb<u8>, Vec<u8>> = ImageBuffer::new(img_w, img_h);

    for row in 0..height as u32 {
        for col in 0..width as u32 {
            let idx = (row * width as u32 + col) as usize;
            let cell = &content[idx];
            let style = cell.style();
            let fg = color_to_rgb(style.fg.unwrap_or(Color::White));
            let bg = color_to_rgb(style.bg.unwrap_or(Color::Black));
            let ch = cell.symbol().chars().next().unwrap_or(' ');
            paint_cell(&mut img, col, row, ch, fg, bg);
        }
    }

    let dyn_img = DynamicImage::ImageRgb8(img);
    let mut png_bytes: Vec<u8> = Vec::new();
    dyn_img
        .write_to(
            &mut std::io::Cursor::new(&mut png_bytes),
            image::ImageFormat::Png,
        )
        .expect("PNG encode");

    (png_bytes, text)
}

/// Paint one character cell into `img` at grid position (`col`, `row`).
///
/// Background colour fills the whole block; non-space characters add a
/// centred foreground rectangle so the colour layout is visible without
/// requiring an embedded font.
fn paint_cell(
    img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    col: u32,
    row: u32,
    ch: char,
    fg: [u8; 3],
    bg: [u8; 3],
) {
    let x0 = col * CELL_W;
    let y0 = row * CELL_H;

    // Background fill.
    for dy in 0..CELL_H {
        for dx in 0..CELL_W {
            img.put_pixel(x0 + dx, y0 + dy, Rgb(bg));
        }
    }

    if matches!(ch, ' ' | '\0') {
        return;
    }

    // Foreground indicator: centred rectangle occupying roughly 60% of the
    // cell, so UI structure (borders, highlights, keyboard keys) is legible.
    let (fx, fy, fw, fh) = fg_rect(ch);
    for dy in fy..fy + fh {
        for dx in fx..fx + fw {
            if dx < CELL_W && dy < CELL_H {
                img.put_pixel(x0 + dx, y0 + dy, Rgb(fg));
            }
        }
    }
}

/// Foreground rectangle (x, y, w, h) within a cell for a given character.
///
/// Block and half-block Unicode are sized to match their semantic coverage;
/// everything else uses a centred square.
fn fg_rect(ch: char) -> (u32, u32, u32, u32) {
    match ch {
        '\u{2588}' => (0, 0, CELL_W, CELL_H), // █ full block
        '\u{2590}' => (CELL_W / 2, 0, CELL_W / 2, CELL_H), // ▐ right half block
        '\u{258C}' => (0, 0, CELL_W / 2, CELL_H), // ▌ left half block
        '\u{2580}' => (0, 0, CELL_W, CELL_H / 2), // ▀ upper half block
        '\u{2584}' => (0, CELL_H / 2, CELL_W, CELL_H / 2), // ▄ lower half block
        '\u{2500}'..='\u{257F}' => (0, CELL_H / 2 - 1, CELL_W, 2), // ─ box-drawing horizontal
        _ => (1, 3, CELL_W - 2, CELL_H - 6),  // general: centred rect
    }
}

/// Map a ratatui [`Color`] to an sRGB triple.
fn color_to_rgb(color: Color) -> [u8; 3] {
    match color {
        Color::Reset | Color::Black => [0, 0, 0],
        Color::Red => [170, 0, 0],
        Color::Green => [0, 170, 0],
        Color::Yellow => [170, 170, 0],
        Color::Blue => [0, 0, 170],
        Color::Magenta => [170, 0, 170],
        Color::Cyan => [0, 170, 170],
        Color::Gray => [170, 170, 170], // ANSI White (foreground 37)
        Color::DarkGray => [85, 85, 85],
        Color::LightRed => [255, 85, 85],
        Color::LightGreen => [85, 255, 85],
        Color::LightYellow => [255, 255, 85],
        Color::LightBlue => [85, 85, 255],
        Color::LightMagenta => [255, 85, 255],
        Color::LightCyan => [85, 255, 255],
        Color::White => [255, 255, 255], // ANSI Bright White (foreground 97)
        Color::Rgb(r, g, b) => [r, g, b],
        Color::Indexed(n) => indexed_to_rgb(n),
    }
}

/// Map a 256-colour palette index to sRGB.
fn indexed_to_rgb(n: u8) -> [u8; 3] {
    match n {
        0 => [0, 0, 0],
        1 => [170, 0, 0],
        2 => [0, 170, 0],
        3 => [170, 170, 0],
        4 => [0, 0, 170],
        5 => [170, 0, 170],
        6 => [0, 170, 170],
        7 => [170, 170, 170],
        8 => [85, 85, 85],
        9 => [255, 85, 85],
        10 => [85, 255, 85],
        11 => [255, 255, 85],
        12 => [85, 85, 255],
        13 => [255, 85, 255],
        14 => [85, 255, 255],
        15 => [255, 255, 255],
        16..=231 => {
            let idx = n - 16;
            let b = (idx % 6) * 51;
            let g = ((idx / 6) % 6) * 51;
            let r = (idx / 36) * 51;
            [r, g, b]
        }
        232..=255 => {
            let v = (n - 232) * 10 + 8;
            [v, v, v]
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_midi::ScriptedSource;

    /// Smoke test: the menu view produces a valid PNG of the correct pixel
    /// dimensions and a non-empty text dump.
    ///
    /// Run with: `cargo test --features screenshot`
    /// (excluded from the default `cargo test --workspace` path)
    #[test]
    fn screenshot_smoke() {
        let shell = Shell::new(Box::new(ScriptedSource::new(vec![])), None, None);
        let (png_bytes, text) = render_to_png(&shell, 80, 24);

        // Must start with the PNG magic bytes.
        assert_eq!(
            &png_bytes[..4],
            &[137, 80, 78, 71],
            "output must start with PNG magic"
        );

        // Correct pixel dimensions.
        let img = image::load_from_memory(&png_bytes).expect("decode PNG");
        assert_eq!(img.width(), 80 * CELL_W, "PNG width = cols × CELL_W");
        assert_eq!(img.height(), 24 * CELL_H, "PNG height = rows × CELL_H");

        // Text dump: exactly 24 lines, non-empty content.
        let lines: Vec<&str> = text.split('\n').collect();
        assert_eq!(lines.len(), 24, "text dump must have exactly 24 lines");
        assert!(
            text.chars().any(|c| !c.is_whitespace()),
            "text dump must contain non-whitespace characters"
        );
    }
}
