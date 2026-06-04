//! Edit screen: the piano-free composer view.
//!
//! Renders a [`Timeline`] on the existing note-highway projection with a movable
//! `(pitch, step)` cursor navigated vim-style across all 88 keys. This is the
//! shell every other M3 composer task plugs into.
//!
//! **Navigation + rendering only.** Note mutation (add/remove/resize) is a later
//! task (#53); unmapped keys are no-ops here. Vertical (time) placement reuses
//! [`crate::highway::project`]; horizontal (pitch) placement reuses the column
//! geometry in [`crate::keyboard`]. Nothing here touches a device or the disk,
//! so the whole screen is headless-testable via the existing `TestBackend`
//! harness.

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rockcraft_core::{Grid, MidiNote, Timeline};

use crate::highway::{build_spans, project, NoteSpan};
use crate::keyboard::{black_key_col, is_black_key, white_index, Scale, HIGHEST_MIDI, LOWEST_MIDI};
use crate::render::draw_keyboard;

/// How many bars of time the highway shows from bottom (keyboard line) to top.
const LEAD_BARS: u64 = 4;

/// Where the cursor sits in the visible window: a quarter of the way up from the
/// keyboard line, leaving a little context below it and most of the window ahead.
const CURSOR_ANCHOR_NUM: u64 = 1;
const CURSOR_ANCHOR_DEN: u64 = 4;

/// One semitone above A0 × octave for the default cursor: middle C (MIDI 60).
const DEFAULT_CURSOR_PITCH: u8 = 60;

/// Cursor highlight (status badge, cursor key, cursor cell).
const CURSOR_COLOR: Color = Color::Magenta;
/// Resting colour for timeline notes on the highway.
const NOTE_COLOR: Color = Color::Indexed(33);
/// Faint colour for beat/bar gridlines.
const GRID_COLOR: Color = Color::DarkGray;

/// A `(pitch, step)` editing cursor.
///
/// `pitch` is a MIDI note constrained to the 88-key range `21..=108`; `step` is
/// a grid-step index along the time axis (its microsecond position is
/// `grid.us_of_step(step)`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cursor {
    pub pitch: u8,
    pub step: u64,
}

/// The composer edit screen: a timeline rendered on the highway with a navigable
/// cursor. Owns its key routing for the navigation keymap (later tasks extend
/// the table — see the spec / issue #52).
pub struct EditScreen {
    timeline: Timeline,
    grid: Grid,
    cursor: Cursor,
}

impl EditScreen {
    /// A fresh editor: empty timeline, default 120 BPM 4/4 grid, cursor parked
    /// at middle C and the song start.
    pub fn new() -> Self {
        Self::from_timeline(Timeline::new(), Grid::default_120())
    }

    /// An editor over an existing timeline and grid. The cursor starts at middle
    /// C and the song start, same as [`EditScreen::new`].
    pub fn from_timeline(timeline: Timeline, grid: Grid) -> Self {
        Self {
            timeline,
            grid,
            cursor: Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0,
            },
        }
    }

    /// The current cursor position (for tests and status rendering).
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    // ── navigation ────────────────────────────────────────────────────────

    /// Route a key press through the navigation keymap. Unmapped keys are
    /// no-ops (note mutation is #53). Tab/Esc are handled by the shell, not here.
    pub fn on_key(&mut self, code: KeyCode) {
        match code {
            // Step left / right (clamp left at 0; right is unbounded).
            KeyCode::Char('h') | KeyCode::Left => {
                self.cursor.step = self.cursor.step.saturating_sub(1);
            }
            KeyCode::Char('l') | KeyCode::Right => {
                self.cursor.step += 1;
            }
            // Semitone down / up (clamp to the 88-key range).
            KeyCode::Char('j') | KeyCode::Down => {
                self.cursor.pitch = self.cursor.pitch.saturating_sub(1).max(LOWEST_MIDI);
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.cursor.pitch = (self.cursor.pitch + 1).min(HIGHEST_MIDI);
            }
            // One bar left / right.
            KeyCode::Char('H') => {
                self.cursor.step = self.cursor.step.saturating_sub(self.steps_per_bar());
            }
            KeyCode::Char('L') => {
                self.cursor.step += self.steps_per_bar();
            }
            // One octave down / up (clamp to the 88-key range).
            KeyCode::Char('J') => {
                self.cursor.pitch = self.cursor.pitch.saturating_sub(12).max(LOWEST_MIDI);
            }
            KeyCode::Char('K') => {
                self.cursor.pitch = (self.cursor.pitch + 12).min(HIGHEST_MIDI);
            }
            // Song start / last note end.
            KeyCode::Char('0') => {
                self.cursor.step = 0;
            }
            KeyCode::Char('$') => {
                self.cursor.step = self.last_step();
            }
            _ => {}
        }
    }

    /// Steps per bar = `bar_us / step_us` (at least 1).
    fn steps_per_bar(&self) -> u64 {
        (self.grid.bar_us() / self.grid.step_us()).max(1)
    }

    /// Grid step of the last note's end (0 for an empty timeline) — the `$` jump.
    fn last_step(&self) -> u64 {
        let end_us = self
            .timeline
            .notes()
            .map(|(_, n)| n.start_us + n.dur_us)
            .max()
            .unwrap_or(0);
        self.grid.step_index(end_us)
    }

    // ── time-axis viewport ────────────────────────────────────────────────

    /// Microsecond position of the cursor on the time axis.
    fn cursor_us(&self) -> u64 {
        self.grid.us_of_step(self.cursor.step)
    }

    /// How far into the future the top of the highway represents.
    fn lead_us(&self) -> u64 {
        (self.grid.bar_us() * LEAD_BARS).max(1)
    }

    /// The time at the bottom (keyboard line) of the highway, scrolled so the
    /// cursor stays anchored a quarter of the way up the visible window.
    fn view_now_us(&self) -> u64 {
        let lead = self.lead_us();
        self.cursor_us()
            .saturating_sub(lead * CURSOR_ANCHOR_NUM / CURSOR_ANCHOR_DEN)
    }

    // ── rendering ─────────────────────────────────────────────────────────

    /// Draw the edit screen: status line, note highway, and the 88-key board.
    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(1), // status
            Constraint::Min(3),    // highway
            Constraint::Length(4), // keyboard
        ])
        .split(area);

        self.draw_status(f, chunks[0]);

        // Draw the keyboard first to learn the scale + left edge so the highway
        // aligns to the exact same columns. Highlight the cursor's key.
        let kb_block = Block::default()
            .borders(Borders::ALL)
            .title(" keyboard (88) ");
        let kb_inner = kb_block.inner(chunks[2]);
        f.render_widget(kb_block, chunks[2]);
        let cursor_pitch = self.cursor.pitch;
        let layout = draw_keyboard(f, kb_inner, &|note| {
            (note == cursor_pitch).then_some(CURSOR_COLOR)
        });

        let hw_block = Block::default().borders(Borders::ALL).title(" edit ");
        let hw_inner = hw_block.inner(chunks[1]);
        f.render_widget(hw_block, chunks[1]);
        if let Some((scale, x0)) = layout {
            self.draw_highway(f, hw_inner, scale, x0);
        }
    }

    fn draw_status(&self, f: &mut Frame, area: Rect) {
        let (bar, beat) = self.grid.bar_beat_of(self.cursor_us());
        let pitch_name = MidiNote::new(self.cursor.pitch)
            .map(|n| n.name())
            .unwrap_or_default();
        let line = Line::from(vec![
            Span::styled(" EDIT ", Style::default().fg(Color::Black).bg(CURSOR_COLOR)),
            // Bars and beats are 1-based for display.
            Span::raw(format!("  bar {}:{}  ", bar + 1, beat + 1)),
            Span::raw(format!("snap {}  ", self.grid.subdivision.label())),
            Span::styled(
                format!("♪ {pitch_name}  "),
                Style::default().fg(CURSOR_COLOR),
            ),
            Span::styled(
                "[hjkl] move  [HJKL] bar/oct  [0/$] ends  [Tab] menu",
                Style::default().fg(Color::DarkGray),
            ),
        ]);
        f.render_widget(Paragraph::new(line), area);
    }

    fn draw_highway(&self, f: &mut Frame, area: Rect, scale: Scale, x0: u16) {
        if area.height == 0 {
            return;
        }
        let w = scale.white_width();
        let now = self.view_now_us();
        let lead = self.lead_us();

        // Column for a note's left edge on the highway, matching keyboard layout.
        let note_col = |note: u8| -> Option<u16> {
            if let Some(wi) = white_index(note) {
                Some(x0 + wi as u16 * w)
            } else if is_black_key(note) {
                black_key_col(note, scale).map(|c| x0 + c)
            } else {
                None
            }
        };

        // Beat/bar gridlines first, so notes and the cursor paint over them.
        self.draw_gridlines(f, area, now, lead);

        // Timeline notes.
        for span in build_spans(&self.timeline.to_events()) {
            let Some(rs) = project(&span, now, lead, area.height) else {
                continue;
            };
            let Some(col) = note_col(span.note) else {
                continue;
            };
            let cell_w = if is_black_key(span.note) { 1 } else { w };
            let glyph = "▓".repeat(cell_w as usize);
            for row in rs.top_row..=rs.bottom_row {
                let y = area.y + row;
                if y >= area.y + area.height {
                    break;
                }
                let rect = Rect::new(col, y, cell_w, 1);
                f.render_widget(
                    Paragraph::new(glyph.clone()).style(Style::default().fg(NOTE_COLOR)),
                    rect,
                );
            }
        }

        // The cursor cell, on top of everything.
        if let Some(col) = note_col(self.cursor.pitch) {
            let cur = NoteSpan {
                note: self.cursor.pitch,
                start_us: self.cursor_us(),
                end_us: self.cursor_us() + 1,
            };
            if let Some(rs) = project(&cur, now, lead, area.height) {
                let cell_w = if is_black_key(self.cursor.pitch) {
                    1
                } else {
                    w
                };
                let y = area.y + rs.bottom_row;
                let rect = Rect::new(col, y, cell_w, 1);
                f.render_widget(
                    Paragraph::new("█".repeat(cell_w as usize)).style(
                        Style::default()
                            .fg(CURSOR_COLOR)
                            .add_modifier(Modifier::BOLD),
                    ),
                    rect,
                );
            }
        }
    }

    /// Faint horizontal lines at bar boundaries within the visible window.
    fn draw_gridlines(&self, f: &mut Frame, area: Rect, now: u64, lead: u64) {
        let bar = self.grid.bar_us();
        if bar == 0 || area.width == 0 {
            return;
        }
        let window_end = now + lead;
        // First bar boundary at or after `now`.
        let mut t = now.div_ceil(bar) * bar;
        let line = "─".repeat(area.width as usize);
        while t <= window_end {
            let marker = NoteSpan {
                note: LOWEST_MIDI,
                start_us: t,
                end_us: t + 1,
            };
            if let Some(rs) = project(&marker, now, lead, area.height) {
                let rect = Rect::new(area.x, area.y + rs.bottom_row, area.width, 1);
                f.render_widget(
                    Paragraph::new(line.clone()).style(Style::default().fg(GRID_COLOR)),
                    rect,
                );
            }
            t += bar;
        }
    }
}

impl Default for EditScreen {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    use rockcraft_core::{Note, Velocity};

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
        }
    }

    #[test]
    fn fresh_cursor_starts_at_middle_c_step_zero() {
        let e = EditScreen::new();
        assert_eq!(
            e.cursor(),
            Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0
            }
        );
    }

    #[test]
    fn h_at_step_zero_stays_at_zero() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('h'));
        assert_eq!(e.cursor().step, 0);
        e.on_key(KeyCode::Left);
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn l_then_h_returns_to_start_step() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('l'));
        assert_eq!(e.cursor().step, 1);
        e.on_key(KeyCode::Char('h'));
        assert_eq!(e.cursor().step, 0);
        // Arrow aliases behave the same.
        e.on_key(KeyCode::Right);
        e.on_key(KeyCode::Left);
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn k_and_j_move_pitch_by_one() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('k'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 1);
        e.on_key(KeyCode::Char('j'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH);
    }

    #[test]
    fn k_clamps_at_top_108() {
        let mut e = EditScreen::new();
        for _ in 0..200 {
            e.on_key(KeyCode::Char('k'));
        }
        assert_eq!(e.cursor().pitch, HIGHEST_MIDI);
    }

    #[test]
    fn j_clamps_at_bottom_21() {
        let mut e = EditScreen::new();
        for _ in 0..200 {
            e.on_key(KeyCode::Char('j'));
        }
        assert_eq!(e.cursor().pitch, LOWEST_MIDI);
    }

    #[test]
    fn octave_jumps_move_by_twelve_and_clamp() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('K'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 12);
        e.on_key(KeyCode::Char('J'));
        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH);

        // Clamp at the top.
        for _ in 0..20 {
            e.on_key(KeyCode::Char('K'));
        }
        assert_eq!(e.cursor().pitch, HIGHEST_MIDI);
        // Clamp at the bottom.
        for _ in 0..20 {
            e.on_key(KeyCode::Char('J'));
        }
        assert_eq!(e.cursor().pitch, LOWEST_MIDI);
    }

    #[test]
    fn bar_jumps_move_by_exactly_steps_per_bar() {
        let mut e = EditScreen::new();
        let steps_per_bar = e.grid.bar_us() / e.grid.step_us();
        assert_eq!(steps_per_bar, 16); // default 120 BPM, 4/4, 1/16 grid

        e.on_key(KeyCode::Char('L'));
        assert_eq!(e.cursor().step, steps_per_bar);
        e.on_key(KeyCode::Char('L'));
        assert_eq!(e.cursor().step, steps_per_bar * 2);
        e.on_key(KeyCode::Char('H'));
        assert_eq!(e.cursor().step, steps_per_bar);
        // H clamps at 0, never underflowing.
        e.on_key(KeyCode::Char('H'));
        e.on_key(KeyCode::Char('H'));
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn zero_and_dollar_jump_to_song_start_and_last_note_end() {
        let mut tl = Timeline::new();
        // Last note ends at 3_000_000 us → step index at 1/16 (125_000 us) = 24.
        tl.insert(note(60, 0, 1_000_000));
        tl.insert(note(64, 2_000_000, 1_000_000));
        let mut e = EditScreen::from_timeline(tl, Grid::default_120());

        e.on_key(KeyCode::Char('$'));
        assert_eq!(e.cursor().step, 24);
        e.on_key(KeyCode::Char('0'));
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn dollar_on_empty_timeline_is_step_zero() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('l')); // move off zero first
        e.on_key(KeyCode::Char('$'));
        assert_eq!(e.cursor().step, 0);
    }

    #[test]
    fn unmapped_keys_are_no_ops() {
        let mut e = EditScreen::new();
        let before = e.cursor();
        e.on_key(KeyCode::Char('z'));
        e.on_key(KeyCode::Char('x'));
        e.on_key(KeyCode::Enter);
        assert_eq!(e.cursor(), before);
    }

    #[test]
    fn from_timeline_renders_a_known_note_marker_without_panic() {
        let mut tl = Timeline::new();
        // A note at pitch 64, away from the cursor's default column (60), so its
        // glyph isn't overwritten by the cursor cell.
        tl.insert(note(64, 0, 500_000));
        let e = EditScreen::from_timeline(tl, Grid::default_120());

        let mut terminal = Terminal::new(TestBackend::new(120, 30)).unwrap();
        terminal
            .draw(|f| e.draw(f, f.area()))
            .expect("draw panicked");

        let buf = terminal.backend().buffer();
        let has_note = buf.content().iter().any(|c| c.symbol() == "▓");
        assert!(has_note, "expected the timeline note's marker to render");
    }
}
