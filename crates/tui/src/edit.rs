//! Edit screen: the piano-free composer view.
//!
//! Renders a [`Timeline`] on the existing note-highway projection with a movable
//! `(pitch, step)` cursor navigated vim-style across all 88 keys. Supports full
//! note editing: add, delete, resize, move (grab), and velocity adjust.
//!
//! All mutations go through [`Timeline`] ops so a future undo layer (#61) can
//! wrap them without changing this module.
//!
//! Nothing here touches a device or the disk, so the whole screen is
//! headless-testable via the existing `TestBackend` harness.

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rockcraft_audio::SynthHandle;
use rockcraft_core::{Grid, MidiNote, Note, NoteId, Timeline, Velocity};

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

/// Default velocity for newly added notes.
const DEFAULT_NOTE_VEL: u8 = 80;

/// Velocity step for `+`/`-` adjustments.
const VEL_STEP: u8 = 8;

/// Cursor highlight (status badge, cursor key, cursor cell).
const CURSOR_COLOR: Color = Color::Magenta;
/// Grab-mode badge colour.
const GRAB_COLOR: Color = Color::Yellow;
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
/// cursor.  Supports vim-style navigation (#52) and note-mutation ops (#53).
pub struct EditScreen {
    timeline: Timeline,
    grid: Grid,
    cursor: Cursor,
    /// The note currently held in grab mode; `None` when not grabbing.
    grabbed: Option<NoteId>,
    /// Optional synth for auditioning notes on add / move.
    synth: Option<SynthHandle>,
    /// The pitch currently sounding from an audition; stopped before the next one.
    auditioning: Option<MidiNote>,
}

impl EditScreen {
    /// A fresh editor: empty timeline, default 120 BPM 4/4 grid, cursor parked
    /// at middle C and the song start.
    pub fn new() -> Self {
        Self::from_parts(Timeline::new(), Grid::default_120(), None)
    }

    /// An editor over an existing timeline and grid. The cursor starts at middle
    /// C and the song start, same as [`EditScreen::new`].
    pub fn from_timeline(timeline: Timeline, grid: Grid) -> Self {
        Self::from_parts(timeline, grid, None)
    }

    fn from_parts(timeline: Timeline, grid: Grid, synth: Option<SynthHandle>) -> Self {
        Self {
            timeline,
            grid,
            cursor: Cursor {
                pitch: DEFAULT_CURSOR_PITCH,
                step: 0,
            },
            grabbed: None,
            synth,
            auditioning: None,
        }
    }

    /// Attach a synth handle so edits are auditioned. Called by the shell after
    /// construction so the existing no-arg `new()` stays usable in tests.
    pub fn attach_synth(&mut self, synth: SynthHandle) {
        self.synth = Some(synth);
    }

    /// Stop any in-progress audition. Call this before navigating away from the
    /// screen so held notes don't linger.
    pub fn leave(&mut self) {
        let prev = self.auditioning.take();
        if let (Some(synth), Some(p)) = (&self.synth, prev) {
            synth.note_off(p);
        }
    }

    // ── read-only accessors ───────────────────────────────────────────────

    /// The current cursor position (for tests and status rendering).
    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Total number of notes in the timeline.
    pub fn note_count(&self) -> usize {
        self.timeline.len()
    }

    /// The id of the note whose span `[start, start+dur)` covers the cursor's
    /// current `(pitch, step)`, if any.
    pub fn note_under_cursor(&self) -> Option<NoteId> {
        self.timeline.find_at(self.cursor.pitch, self.cursor_us())
    }

    /// Look up note data by id (convenience for tests and status display).
    pub fn get_note(&self, id: NoteId) -> Option<Note> {
        self.timeline.get(id).copied()
    }

    // ── key routing ───────────────────────────────────────────────────────

    /// Route a key press through the full keymap. Tab/Esc are handled by the
    /// shell, not here.
    pub fn on_key(&mut self, code: KeyCode) {
        match code {
            // ── navigation ──────────────────────────────────────────────
            // Step left / right (clamp left at 0; right is unbounded).
            // In grab mode, h/l move the grabbed note's start instead of just
            // the cursor; the cursor tracks along with the note.
            KeyCode::Char('h') | KeyCode::Left => {
                if let Some(id) = self.grabbed {
                    let new_step = self.cursor.step.saturating_sub(1);
                    self.timeline.set_start(id, self.grid.us_of_step(new_step));
                    self.cursor.step = new_step;
                    self.audition_note(id);
                } else {
                    self.cursor.step = self.cursor.step.saturating_sub(1);
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if let Some(id) = self.grabbed {
                    let new_step = self.cursor.step + 1;
                    self.timeline.set_start(id, self.grid.us_of_step(new_step));
                    self.cursor.step = new_step;
                    self.audition_note(id);
                } else {
                    self.cursor.step += 1;
                }
            }
            // Semitone down / up (clamp to the 88-key range).
            // In grab mode, j/k transpose the grabbed note; cursor tracks.
            KeyCode::Char('j') | KeyCode::Down => {
                if let Some(id) = self.grabbed {
                    if self.timeline.transpose(id, -1) {
                        self.cursor.pitch = self.cursor.pitch.saturating_sub(1).max(LOWEST_MIDI);
                    }
                    self.audition_note(id);
                } else {
                    self.cursor.pitch = self.cursor.pitch.saturating_sub(1).max(LOWEST_MIDI);
                }
            }
            KeyCode::Char('k') | KeyCode::Up => {
                if let Some(id) = self.grabbed {
                    if self.timeline.transpose(id, 1) {
                        self.cursor.pitch = (self.cursor.pitch + 1).min(HIGHEST_MIDI);
                    }
                    self.audition_note(id);
                } else {
                    self.cursor.pitch = (self.cursor.pitch + 1).min(HIGHEST_MIDI);
                }
            }
            // One bar left / right (navigation only; no grab movement).
            KeyCode::Char('H') => {
                self.cursor.step = self.cursor.step.saturating_sub(self.steps_per_bar());
            }
            KeyCode::Char('L') => {
                self.cursor.step += self.steps_per_bar();
            }
            // One octave down / up (navigation only; no grab movement).
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

            // ── edit operations ──────────────────────────────────────────
            // Add a note at cursor (pitch=cursor, start=cursor_us, dur=1 step,
            // vel=80). If the cell is already occupied the existing note is
            // removed first — replace / re-trigger semantics.
            KeyCode::Char('a') | KeyCode::Char('i') => {
                self.add_note();
            }
            // Delete the note under the cursor; no-op on an empty cell.
            KeyCode::Char('x') | KeyCode::Char('d') => {
                self.delete_note();
            }
            // Lengthen / shorten the note under the cursor by one grid step.
            // Shorten clamps at a minimum of one step.
            KeyCode::Char(']') => {
                self.resize_note(1);
            }
            KeyCode::Char('[') => {
                self.resize_note(-1);
            }
            // Velocity +8 / −8, clamped to 1..=127.
            KeyCode::Char('+') | KeyCode::Char('=') => {
                self.adjust_velocity(VEL_STEP as i16);
            }
            KeyCode::Char('-') => {
                self.adjust_velocity(-(VEL_STEP as i16));
            }
            // Toggle grab mode: h/j/k/l move the grabbed note; `m` again drops.
            KeyCode::Char('m') => {
                self.toggle_grab();
            }

            _ => {}
        }
    }

    // ── edit helpers ──────────────────────────────────────────────────────

    /// Add a note at the cursor. If the cell is occupied the existing note is
    /// removed first (replace semantics: the new note wins, velocity resets to
    /// the default 80, duration resets to one step).
    fn add_note(&mut self) {
        if let Some(id) = self.note_under_cursor() {
            self.timeline.remove(id);
            if self.grabbed == Some(id) {
                self.grabbed = None;
            }
        }
        let pitch = MidiNote::new(self.cursor.pitch).expect("cursor pitch is always valid");
        let velocity = Velocity::new(DEFAULT_NOTE_VEL).expect("80 is always valid");
        let note = Note {
            pitch,
            start_us: self.cursor_us(),
            dur_us: self.grid.step_us(),
            velocity,
        };
        self.timeline.insert(note);
        self.audition(pitch, velocity);
    }

    /// Delete the note under the cursor. No-op if the cell is empty.
    fn delete_note(&mut self) {
        if let Some(id) = self.note_under_cursor() {
            self.timeline.remove(id);
            if self.grabbed == Some(id) {
                self.grabbed = None;
            }
        }
    }

    /// Resize the note under the cursor by `delta_steps` grid steps. Positive
    /// lengthens; negative shortens, clamped at one step minimum.
    fn resize_note(&mut self, delta_steps: i64) {
        let Some(id) = self.note_under_cursor() else {
            return;
        };
        let Some(note) = self.timeline.get(id).copied() else {
            return;
        };
        let step = self.grid.step_us();
        let new_dur = if delta_steps >= 0 {
            note.dur_us.saturating_add(step * delta_steps as u64)
        } else {
            note.dur_us
                .saturating_sub(step * (-delta_steps) as u64)
                .max(step)
        };
        self.timeline.resize(id, new_dur);
    }

    /// Adjust velocity on the note under the cursor by `delta`, clamped to
    /// `1..=127`. Because `Timeline` has no `set_velocity`, the note is
    /// removed and re-inserted; if it was grabbed the grab follows the new id.
    fn adjust_velocity(&mut self, delta: i16) {
        let Some(id) = self.note_under_cursor() else {
            return;
        };
        let Some(note) = self.timeline.get(id).copied() else {
            return;
        };
        let new_vel = (note.velocity.value() as i16 + delta).clamp(1, 127) as u8;
        let new_note = Note {
            velocity: Velocity::new(new_vel).expect("clamped to 1..=127"),
            ..note
        };
        self.timeline.remove(id);
        let new_id = self.timeline.insert(new_note);
        if self.grabbed == Some(id) {
            self.grabbed = Some(new_id);
        }
    }

    /// Toggle grab mode. While grabbing, h/j/k/l move the held note instead
    /// of navigating the cursor (the cursor tracks the note). A second `m`
    /// drops the grab. `m` on an empty cell is a no-op.
    fn toggle_grab(&mut self) {
        if self.grabbed.is_some() {
            self.grabbed = None;
        } else if let Some(id) = self.note_under_cursor() {
            self.grabbed = Some(id);
            self.audition_note(id);
        }
    }

    // ── audition ─────────────────────────────────────────────────────────

    /// Audition the note identified by `id`: stops the previous audition first,
    /// then plays a note-on. No-op when no synth is attached.
    fn audition_note(&mut self, id: NoteId) {
        let Some(note) = self.timeline.get(id).copied() else {
            return;
        };
        self.audition(note.pitch, note.velocity);
    }

    /// Stop any previous audition and start a new one.
    fn audition(&mut self, pitch: MidiNote, velocity: Velocity) {
        let prev = self.auditioning.take();
        // Clone the handle so there are no outstanding borrows while we
        // mutate `auditioning`.
        let Some(synth) = self.synth.clone() else {
            return;
        };
        if let Some(p) = prev {
            synth.note_off(p);
        }
        synth.note_on(pitch, velocity);
        self.auditioning = Some(pitch);
    }

    // ── navigation helpers ────────────────────────────────────────────────

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

        let (badge_text, badge_style) = if self.grabbed.is_some() {
            (" GRAB ", Style::default().fg(Color::Black).bg(GRAB_COLOR))
        } else {
            (" EDIT ", Style::default().fg(Color::Black).bg(CURSOR_COLOR))
        };

        let line = Line::from(vec![
            Span::styled(badge_text, badge_style),
            Span::raw(format!("  bar {}:{}  ", bar + 1, beat + 1)),
            Span::raw(format!("snap {}  ", self.grid.subdivision.label())),
            Span::styled(
                format!("♪ {pitch_name}  "),
                Style::default().fg(CURSOR_COLOR),
            ),
            Span::styled(
                "[a/x] add/del  []/[] size  [+/-] vel  [m] grab  [hjkl] move  [HJKL] bar/oct  [0/$] ends  [Tab] menu",
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

    fn note(pitch: u8, start_us: u64, dur_us: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us,
            dur_us,
            velocity: Velocity::new(80).unwrap(),
        }
    }

    // ── existing navigation tests ────────────────────────────────────────

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
        e.on_key(KeyCode::Char('x')); // delete on empty cell → cursor unchanged
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

    // ── add / delete tests ───────────────────────────────────────────────

    /// `a` inserts exactly one note; properties match the cursor's pitch/step
    /// with a one-step duration and the default velocity.
    #[test]
    fn a_adds_note_at_cursor() {
        let mut e = EditScreen::new();
        assert_eq!(e.note_count(), 0);

        e.on_key(KeyCode::Char('a'));

        assert_eq!(e.note_count(), 1);
        let id = e.note_under_cursor().expect("note at cursor after add");
        let n = e.get_note(id).unwrap();
        assert_eq!(n.pitch.value(), DEFAULT_CURSOR_PITCH);
        assert_eq!(n.start_us, 0); // step 0 → 0 µs
        assert_eq!(n.dur_us, e.grid.step_us()); // one grid step
        assert_eq!(n.velocity.value(), DEFAULT_NOTE_VEL);
    }

    /// `i` is an alias for `a`.
    #[test]
    fn i_is_alias_for_add() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('i'));
        assert_eq!(e.note_count(), 1);
    }

    /// `a` on an occupied cell replaces it: count stays at 1, the new note
    /// resets to default velocity and a one-step duration.
    #[test]
    fn a_on_occupied_cell_replaces_note() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add first note (vel=80, dur=1 step)
                                      // Lengthen and change velocity so we can tell the difference.
        e.on_key(KeyCode::Char(']')); // dur → 2 steps
        e.on_key(KeyCode::Char('+')); // vel → 88
        assert_eq!(e.note_count(), 1);
        {
            let id = e.note_under_cursor().unwrap();
            let n = e.get_note(id).unwrap();
            assert_eq!(n.dur_us, e.grid.step_us() * 2);
            assert_eq!(n.velocity.value(), 88);
        }

        // Add again on the same cell: replaces.
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1, "still exactly one note after replace");
        let id = e.note_under_cursor().unwrap();
        let n = e.get_note(id).unwrap();
        assert_eq!(n.dur_us, e.grid.step_us(), "duration reset to one step");
        assert_eq!(
            n.velocity.value(),
            DEFAULT_NOTE_VEL,
            "velocity reset to default"
        );
    }

    /// `x` removes the note under the cursor.
    #[test]
    fn x_deletes_note_under_cursor() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        assert_eq!(e.note_count(), 1);

        e.on_key(KeyCode::Char('x'));

        assert_eq!(e.note_count(), 0);
        assert!(e.note_under_cursor().is_none());
    }

    /// `d` is an alias for `x`.
    #[test]
    fn d_is_alias_for_delete() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        e.on_key(KeyCode::Char('d'));
        assert_eq!(e.note_count(), 0);
    }

    /// `x` on an empty cell is a no-op (cursor unchanged, no panic).
    #[test]
    fn x_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        let before = e.cursor();
        e.on_key(KeyCode::Char('x'));
        assert_eq!(e.note_count(), 0);
        assert_eq!(e.cursor(), before);
    }

    // ── resize tests ──────────────────────────────────────────────────────

    /// `]` and `[` change duration by one step; `[` never goes below one step.
    #[test]
    fn bracket_keys_resize_note() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a'));
        let step = e.grid.step_us();

        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step, "starts at one step");

        // Lengthen: 1 → 2 steps.
        e.on_key(KeyCode::Char(']'));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step * 2);

        // Shorten: 2 → 1 step.
        e.on_key(KeyCode::Char('['));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step);

        // Shorten again: clamps at one step minimum.
        e.on_key(KeyCode::Char('['));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().dur_us, step, "floor is one step");
    }

    /// `]` and `[` are no-ops on an empty cell.
    #[test]
    fn resize_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char(']'));
        e.on_key(KeyCode::Char('['));
        assert_eq!(e.note_count(), 0);
    }

    // ── velocity tests ────────────────────────────────────────────────────

    /// `+` and `-` adjust velocity by VEL_STEP, clamped to 1..=127.
    #[test]
    fn velocity_keys_adjust_and_clamp() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // vel = 80

        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().velocity.value(), 80);

        // Drive velocity down past zero: should clamp at 1.
        for _ in 0..15 {
            e.on_key(KeyCode::Char('-'));
        }
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().velocity.value(), 1, "clamped at 1");

        // Drive velocity up past 127: should clamp at 127.
        for _ in 0..25 {
            e.on_key(KeyCode::Char('+'));
        }
        let id = e.note_under_cursor().unwrap();
        assert_eq!(
            e.get_note(id).unwrap().velocity.value(),
            127,
            "clamped at 127"
        );
    }

    /// `=` is an alias for `+`.
    #[test]
    fn equals_is_alias_for_velocity_up() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // vel = 80
        e.on_key(KeyCode::Char('='));
        let id = e.note_under_cursor().unwrap();
        assert_eq!(e.get_note(id).unwrap().velocity.value(), 80 + VEL_STEP);
    }

    /// `+`/`-` on an empty cell are no-ops.
    #[test]
    fn velocity_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('+'));
        e.on_key(KeyCode::Char('-'));
        assert_eq!(e.note_count(), 0);
    }

    // ── grab-mode tests ───────────────────────────────────────────────────

    /// `m` + `l` moves the note's start forward by one step and the cursor tracks.
    #[test]
    fn grab_l_moves_note_start_and_cursor_tracks() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add note at step 0

        e.on_key(KeyCode::Char('m')); // grab it

        e.on_key(KeyCode::Char('l')); // move right → step 1
        assert_eq!(e.cursor().step, 1, "cursor follows note");
        let id = e.note_under_cursor().expect("note at new position");
        assert_eq!(
            e.get_note(id).unwrap().start_us,
            e.grid.us_of_step(1),
            "note start moved"
        );
    }

    /// `m` + `k` transposes the note up and the cursor pitch tracks.
    #[test]
    fn grab_k_transposes_note_and_cursor_tracks() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // add note at pitch 60

        e.on_key(KeyCode::Char('m')); // grab
        e.on_key(KeyCode::Char('k')); // transpose up

        assert_eq!(e.cursor().pitch, DEFAULT_CURSOR_PITCH + 1, "cursor up");
        let id = e.note_under_cursor().expect("note at new pitch");
        assert_eq!(
            e.get_note(id).unwrap().pitch.value(),
            DEFAULT_CURSOR_PITCH + 1,
            "note pitch changed"
        );
    }

    /// `m` again drops grab; subsequent navigation moves only the cursor.
    #[test]
    fn grab_dropped_by_second_m() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('a')); // note at step 0

        e.on_key(KeyCode::Char('m')); // grab
        e.on_key(KeyCode::Char('m')); // drop

        // `l` should now only move the cursor, not the note.
        e.on_key(KeyCode::Char('l'));
        assert_eq!(e.cursor().step, 1, "cursor moved");
        // The note is still at step 0 (cursor moved away from it).
        assert!(
            e.note_under_cursor().is_none(),
            "cursor moved off the note after grab drop"
        );
    }

    /// `m` on an empty cell is a no-op (no grab activated).
    #[test]
    fn grab_on_empty_cell_is_noop() {
        let mut e = EditScreen::new();
        e.on_key(KeyCode::Char('m')); // nothing to grab
                                      // Navigation is still cursor-only.
        e.on_key(KeyCode::Char('l'));
        assert_eq!(e.cursor().step, 1);
        assert_eq!(e.note_count(), 0);
    }
}
