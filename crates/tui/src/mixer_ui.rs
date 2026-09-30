//! The mixer overlay (M18-A): a small box over the play and edit screens for
//! setting the level of your notes, the song and the backing, and the two
//! instruments, from the keyboard.
//!
//! This module holds only the view state and the drawing; the shell owns the
//! [`rockcraft_core::Mixer`] and applies each change through
//! `Shell::apply_mixer`, so a change sounds at once and gets remembered.

use crossterm::event::KeyCode;
use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};
use rockcraft_core::{instruments, Mixer, MixerBus, SynthBus};

/// Step for the level rows.
pub const GAIN_STEP: f32 = 0.05;

/// One selectable row of the overlay, top to bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Row {
    YouLevel,
    SongLevel,
    BackingLevel,
    YouInstrument,
    SongInstrument,
}

const ROWS: [Row; 5] = [
    Row::YouLevel,
    Row::SongLevel,
    Row::BackingLevel,
    Row::YouInstrument,
    Row::SongInstrument,
];

/// What one key asks the shell to change in the mix.
#[derive(Debug, Clone, PartialEq)]
pub enum MixChange {
    /// Set a bus level to this value (already clamped by the overlay).
    Gain(MixerBus, f32),
    /// Point a synth bus at this catalog instrument id.
    Instrument(SynthBus, &'static str),
}

/// The overlay's own state: which row is selected.
#[derive(Debug, Clone, Copy, Default)]
pub struct MixerOverlay {
    selected: usize,
}

impl MixerOverlay {
    pub fn new() -> Self {
        Self::default()
    }

    /// The selected row.
    pub fn row(&self) -> Row {
        ROWS[self.selected]
    }

    /// Handle a key. `Ok(true)` means the overlay should close; the optional
    /// change is what the shell should apply to the mix. Unused keys do
    /// nothing (they must not leak to the screen underneath).
    pub fn on_key(&mut self, code: KeyCode, mixer: &Mixer) -> (bool, Option<MixChange>) {
        match code {
            KeyCode::Esc | KeyCode::Char('x') => (true, None),
            KeyCode::Up => {
                self.selected = (self.selected + ROWS.len() - 1) % ROWS.len();
                (false, None)
            }
            KeyCode::Down => {
                self.selected = (self.selected + 1) % ROWS.len();
                (false, None)
            }
            KeyCode::Left => (false, self.nudge(mixer, -1)),
            KeyCode::Right => (false, self.nudge(mixer, 1)),
            KeyCode::Home => (false, self.jump(0.0)),
            KeyCode::End => (false, self.jump(1.0)),
            _ => (false, None),
        }
    }

    fn level_bus(&self) -> Option<MixerBus> {
        match self.row() {
            Row::YouLevel => Some(MixerBus::Player),
            Row::SongLevel => Some(MixerBus::Song),
            Row::BackingLevel => Some(MixerBus::Backing),
            _ => None,
        }
    }

    fn instrument_bus(&self) -> Option<SynthBus> {
        match self.row() {
            Row::YouInstrument => Some(SynthBus::Player),
            Row::SongInstrument => Some(SynthBus::Song),
            _ => None,
        }
    }

    fn jump(&self, value: f32) -> Option<MixChange> {
        self.level_bus().map(|bus| MixChange::Gain(bus, value))
    }

    fn nudge(&self, mixer: &Mixer, dir: i32) -> Option<MixChange> {
        if let Some(bus) = self.level_bus() {
            // Round to the step grid so repeated presses don't drift.
            let next = (mixer.gain(bus).value() / GAIN_STEP).round() + dir as f32;
            let value = (next * GAIN_STEP).clamp(0.0, 1.0);
            return Some(MixChange::Gain(bus, (value * 100.0).round() / 100.0));
        }
        let bus = self.instrument_bus()?;
        let catalog = instruments();
        let current = mixer.bus(bus).instrument.id;
        let at = catalog.iter().position(|i| i.id == current).unwrap_or(0) as i32;
        let n = catalog.len() as i32;
        let next = (at + dir).rem_euclid(n) as usize;
        Some(MixChange::Instrument(bus, catalog[next].id))
    }

    /// Draw the box centred in `area` (clamped to fit small terminals).
    pub fn draw(&self, f: &mut Frame, area: Rect, mixer: &Mixer, note: &str) {
        let width = 46.min(area.width);
        let height = 9.min(area.height);
        let rect = Rect {
            x: area.x + (area.width - width) / 2,
            y: area.y + (area.height - height) / 2,
            width,
            height,
        };
        f.render_widget(Clear, rect);
        let mut lines: Vec<Line> = ROWS
            .iter()
            .enumerate()
            .map(|(i, &row)| {
                let text = row_text(row, mixer);
                let style = if i == self.selected {
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                };
                Line::from(Span::styled(text, style))
            })
            .collect();
        lines.push(Line::from(Span::styled(
            if note.is_empty() {
                "↑↓ row  ←→ change  Home/End 0/1  x/Esc close"
            } else {
                note
            },
            Style::default().fg(if note.is_empty() {
                Color::DarkGray
            } else {
                Color::Yellow
            }),
        )));
        f.render_widget(
            Paragraph::new(lines).block(Block::default().borders(Borders::ALL).title(" mixer ")),
            rect,
        );
    }
}

/// `▓▓▓▓▓▓▓▓░░` style bar for a level.
fn bar(value: f32) -> String {
    let filled = (value * 10.0).round().clamp(0.0, 10.0) as usize;
    format!("{}{}", "█".repeat(filled), "░".repeat(10 - filled))
}

fn row_text(row: Row, mixer: &Mixer) -> String {
    let level = |label: &str, bus: MixerBus| {
        let g = mixer.gain(bus).value();
        format!(" {label:<13}{} {g:.2}", bar(g))
    };
    let sound =
        |label: &str, bus: SynthBus| format!(" {label:<13}◀ {} ▶", mixer.bus(bus).instrument.name);
    match row {
        Row::YouLevel => level("You", MixerBus::Player),
        Row::SongLevel => level("Song", MixerBus::Song),
        Row::BackingLevel => level("Backing", MixerBus::Backing),
        Row::YouInstrument => sound("You sound", SynthBus::Player),
        Row::SongInstrument => sound("Song sound", SynthBus::Song),
    }
}
