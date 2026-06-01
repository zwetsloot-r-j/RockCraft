//! Shared ratatui drawing helpers used by more than one screen.
//!
//! The keyboard is drawn by both Record and Play, so it lives here. Geometry
//! comes from the pure [`crate::keyboard`] module; this only paints it.

use ratatui::{
    layout::Rect,
    style::{Color, Style},
    text::Line,
    widgets::Paragraph,
    Frame,
};

use crate::keyboard::{
    black_key_col, is_black_key, white_key_slots, Scale, HIGHEST_MIDI, LOWEST_MIDI,
};

/// Resting white / black key colours, and the player's held-key highlight.
pub const WHITE_KEY: Color = Color::White;
pub const BLACK_KEY: Color = Color::DarkGray;
pub const HELD_COLOR: Color = Color::Cyan;
/// What the song wants right now (play-along target).
pub const TARGET_COLOR: Color = Color::Yellow;
/// Player hits a key the song also wants right now.
pub const MATCH_COLOR: Color = Color::Green;

/// Draw the 88-key keyboard filling `area`, colouring each key via `key_color`.
/// `key_color(note)` returns `None` to use the default resting colour, or a
/// `Some(color)` override (held, target, etc.).
///
/// Returns the chosen `Scale` and left edge `x0` so a caller (the highway) can
/// align falling notes to the exact same columns, or `None` if too narrow.
pub fn draw_keyboard(
    f: &mut Frame,
    area: Rect,
    key_color: &dyn Fn(u8) -> Option<Color>,
) -> Option<(Scale, u16)> {
    let scale = match Scale::best_for_width(area.width) {
        Some(s) => s,
        None => {
            let msg = format!(
                "Widen terminal to ≥{} columns to show the keyboard",
                Scale::Compact.total_width()
            );
            f.render_widget(Paragraph::new(msg), area);
            return None;
        }
    };

    let board_w = scale.total_width();
    let x0 = area.x + (area.width.saturating_sub(board_w)) / 2;
    let body_h = area.height;

    // White keys.
    for slot in white_key_slots(scale) {
        let color = key_color(slot.note).unwrap_or(WHITE_KEY);
        let cell = "█".repeat(slot.width as usize);
        for dy in 0..body_h {
            let rect = Rect::new(x0 + slot.col, area.y + dy, slot.width, 1);
            f.render_widget(
                Paragraph::new(cell.clone()).style(Style::default().fg(color)),
                rect,
            );
        }
    }

    // Black keys overlaid between whites.
    let black_h = body_h.saturating_sub(1).max(1);
    for note in LOWEST_MIDI..=HIGHEST_MIDI {
        if !is_black_key(note) {
            continue;
        }
        let Some(col) = black_key_col(note, scale) else {
            continue;
        };
        let color = key_color(note).unwrap_or(BLACK_KEY);
        for dy in 0..black_h {
            let rect = Rect::new(x0 + col, area.y + dy, 1, 1);
            f.render_widget(Paragraph::new("▐").style(Style::default().fg(color)), rect);
        }
    }

    Some((scale, x0))
}

/// Render right-aligned-to-bottom log lines inside `area` (already the inner
/// region of a bordered block).
pub fn draw_log_lines<'a>(f: &mut Frame, area: Rect, lines: impl Iterator<Item = &'a String>) {
    let height = area.height as usize;
    let collected: Vec<&String> = lines.collect();
    let start = collected.len().saturating_sub(height);
    let shown: Vec<Line> = collected[start..]
        .iter()
        .map(|s| Line::from(s.as_str()))
        .collect();
    f.render_widget(Paragraph::new(shown), area);
}
