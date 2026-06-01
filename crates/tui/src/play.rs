//! Play screen: a falling-note highway above the keyboard. Loads a `.mid`,
//! scrolls its notes down to the keyboard line on a playback clock, and lights
//! the player's live keys over it (play-along; scoring is a later task).

use std::time::Instant;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rockcraft_core::NoteEvent;
use rockcraft_midi::smf_bytes_to_events;

use crate::highway::{build_spans, project, song_duration_us, NoteSpan};
use crate::keyboard::{black_key_col, is_black_key, white_index, HeldNotes, Scale};
use crate::render::{draw_keyboard, HELD_COLOR, MATCH_COLOR, TARGET_COLOR};

/// How far into the future the top of the highway represents (microseconds).
/// Larger = notes fall more slowly / you see further ahead.
const LEAD_US: u64 = 2_000_000;

/// Extra empty pause before the first note enters the top of the highway, so
/// playback doesn't open with a note already mid-fall. Total time before the
/// first note reaches the keyboard is `PRE_ROLL_US + LEAD_US`.
const PRE_ROLL_US: u64 = 1_500_000;

pub struct PlayScreen {
    spans: Vec<NoteSpan>,
    duration_us: u64,
    held: HeldNotes,
    started: Instant,
    title: String,
    finished_pause_us: u64,
}

impl PlayScreen {
    /// Load a song from `.mid` bytes.
    pub fn from_smf_bytes(title: String, bytes: &[u8]) -> Result<Self, String> {
        let events = smf_bytes_to_events(bytes).map_err(|e| e.to_string())?;
        let raw = build_spans(&events);

        // Shift the whole song forward so the first note starts at
        // PRE_ROLL + LEAD: the highway opens empty, the first note appears at
        // the top after PRE_ROLL, then falls for one lead window. The clock
        // then simply runs from 0. Also makes any song (first note not at t=0)
        // behave identically.
        let first_us = raw.iter().map(|s| s.start_us).min().unwrap_or(0);
        let offset = (PRE_ROLL_US + LEAD_US).saturating_sub(first_us);
        let spans: Vec<NoteSpan> = raw
            .into_iter()
            .map(|s| NoteSpan {
                note: s.note,
                start_us: s.start_us + offset,
                end_us: s.end_us + offset,
            })
            .collect();
        let duration_us = song_duration_us(&spans);

        Ok(Self {
            spans,
            duration_us,
            held: HeldNotes::new(),
            started: Instant::now(),
            title,
            // keep scrolling a little past the end so the last notes land
            finished_pause_us: LEAD_US,
        })
    }

    /// Restart playback from the top.
    pub fn restart(&mut self) {
        self.started = Instant::now();
    }

    pub fn ingest(&mut self, ev: NoteEvent) {
        self.held.apply(&ev);
    }

    /// Current playback time in microseconds since entering the screen.
    fn now_us(&self) -> u64 {
        self.started.elapsed().as_micros() as u64
    }

    /// Has the song (plus tail) finished?
    pub fn is_finished(&self) -> bool {
        self.now_us() > self.duration_us + self.finished_pause_us
    }

    /// Notes the song wants held at the current instant (target set).
    fn targets_now(&self, now: u64) -> Vec<u8> {
        self.spans
            .iter()
            .filter(|s| s.start_us <= now && now < s.end_us)
            .map(|s| s.note)
            .collect()
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let now = self.now_us();

        let chunks = Layout::vertical([
            Constraint::Length(1), // status
            Constraint::Min(3),    // highway
            Constraint::Length(4), // keyboard
        ])
        .split(area);

        self.draw_status(f, chunks[0], now);
        // Draw the keyboard first to learn the scale + x0 to align the highway.
        let kb_block = Block::default()
            .borders(Borders::ALL)
            .title(" keyboard (88) ");
        let kb_inner = kb_block.inner(chunks[2]);
        f.render_widget(kb_block, chunks[2]);

        let targets = self.targets_now(now);
        let held = &self.held;
        let target_set = &targets;
        let layout = draw_keyboard(f, kb_inner, &|note| {
            let is_target = target_set.contains(&note);
            let is_held = held.is_held(note);
            match (is_target, is_held) {
                (true, true) => Some(MATCH_COLOR),   // hitting the right note
                (true, false) => Some(TARGET_COLOR), // song wants this now
                (false, true) => Some(HELD_COLOR),   // you're playing this
                (false, false) => None,
            }
        });

        // Highway, aligned to the same columns as the keyboard.
        let hw_block = Block::default()
            .borders(Borders::ALL)
            .title(format!(" {} ", self.title));
        let hw_inner = hw_block.inner(chunks[1]);
        f.render_widget(hw_block, chunks[1]);
        if let Some((scale, x0)) = layout {
            self.draw_highway(f, hw_inner, scale, x0, now);
        }
    }

    fn draw_status(&self, f: &mut Frame, area: Rect, now: u64) {
        let secs = now as f64 / 1_000_000.0;
        let total = self.duration_us as f64 / 1_000_000.0;
        let line = Line::from(vec![
            Span::styled(" PLAY ", Style::default().fg(Color::Black).bg(TARGET_COLOR)),
            Span::raw(format!("  {:.1}s / {:.1}s  ", secs, total)),
            Span::raw("[r] restart  [Tab] menu  "),
            Span::styled("● target ", Style::default().fg(TARGET_COLOR)),
            Span::styled("● you ", Style::default().fg(HELD_COLOR)),
            Span::styled("● match", Style::default().fg(MATCH_COLOR)),
        ]);
        f.render_widget(Paragraph::new(line), area);
    }

    fn draw_highway(&self, f: &mut Frame, area: Rect, scale: Scale, x0: u16, now: u64) {
        if area.height == 0 {
            return;
        }
        let w = scale.white_width();
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

        for span in &self.spans {
            let Some(rs) = project(span, now, LEAD_US, area.height) else {
                continue;
            };
            let Some(col) = note_col(span.note) else {
                continue;
            };
            let cell_w = if is_black_key(span.note) { 1 } else { w };
            let glyph = "▓".repeat(cell_w as usize);
            // Notes nearer "now" (bottom) brighter; further ahead dimmer.
            for row in rs.top_row..=rs.bottom_row {
                let y = area.y + row;
                if y >= area.y + area.height {
                    break;
                }
                let color = if span.start_us <= now && now < span.end_us {
                    TARGET_COLOR
                } else {
                    Color::Indexed(33) // a calm blue for upcoming notes
                };
                let rect = Rect::new(col, y, cell_w, 1);
                f.render_widget(
                    Paragraph::new(glyph.clone()).style(Style::default().fg(color)),
                    rect,
                );
            }
        }
    }
}
