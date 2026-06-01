//! Record screen: the keyboard + live event log, capturing the session into an
//! `EventBuffer` that can be saved to a `.mid` (the song source for Play mode).

use std::collections::VecDeque;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
    Frame,
};
use rockcraft_core::{EventBuffer, MidiNote, NoteEvent, NoteEventKind};
use rockcraft_midi::events_to_smf_bytes;

use crate::keyboard::HeldNotes;
use crate::render::{draw_keyboard, draw_log_lines, HELD_COLOR};

const LOG_CAPACITY: usize = 200;

/// Where saved takes go. Relative to the working directory.
const RECORDINGS_DIR: &str = "recordings";

pub struct RecordScreen {
    held: HeldNotes,
    buffer: EventBuffer,
    log: VecDeque<String>,
    status: String,
    /// Timestamp of the first event seen. The MIDI clock is relative to app
    /// startup (when `LiveInput` connected), so we rebase every event by this
    /// origin — the first note played becomes t=0, dropping the menu dead time.
    origin_us: Option<u64>,
}

impl RecordScreen {
    pub fn new() -> Self {
        Self {
            held: HeldNotes::new(),
            buffer: EventBuffer::new(),
            log: VecDeque::with_capacity(LOG_CAPACITY),
            status: "recording — [s] save  [Tab] menu".to_string(),
            origin_us: None,
        }
    }

    pub fn ingest(&mut self, ev: NoteEvent) {
        // Rebase so the first event is t=0.
        let origin = *self.origin_us.get_or_insert(ev.timestamp_us);
        let ev = NoteEvent {
            timestamp_us: ev.timestamp_us.saturating_sub(origin),
            ..ev
        };
        self.held.apply(&ev);
        self.buffer.push(ev);
        let name = ev.note.name();
        let line = match ev.kind {
            NoteEventKind::On { velocity } if velocity.value() > 0 => {
                format!(
                    "{:>10}us  ON   {:<4} vel {}",
                    ev.timestamp_us,
                    name,
                    velocity.value()
                )
            }
            _ => format!("{:>10}us  OFF  {}", ev.timestamp_us, name),
        };
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    /// Save the captured session to a timestamped `.mid` under `recordings/`.
    /// Returns the path written, for the caller to surface / hand to Play.
    pub fn save(&mut self) -> std::io::Result<std::path::PathBuf> {
        std::fs::create_dir_all(RECORDINGS_DIR)?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let path = std::path::Path::new(RECORDINGS_DIR).join(format!("take-{stamp}.mid"));
        let bytes = events_to_smf_bytes(self.buffer.events());
        std::fs::write(&path, bytes)?;
        self.status = format!(
            "saved {} ({} events) — [Tab] menu",
            path.display(),
            self.buffer.len()
        );
        Ok(path)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(4),
        ])
        .split(area);

        // Status line.
        let held_text = if self.held.is_empty() {
            "—".to_string()
        } else {
            self.held
                .iter()
                .filter_map(|n| MidiNote::new(n).map(|m| m.name()))
                .collect::<Vec<_>>()
                .join(" ")
        };
        let status = Line::from(vec![
            Span::styled(" RECORD ", Style::default().fg(Color::Black).bg(Color::Red)),
            Span::raw(format!("  {}  ", self.status)),
            Span::raw(format!("held {:<2} ", self.held.len())),
            Span::styled(held_text, Style::default().fg(HELD_COLOR)),
        ]);
        f.render_widget(ratatui::widgets::Paragraph::new(status), chunks[0]);

        // Event log.
        let log_block = Block::default().borders(Borders::ALL).title(" events ");
        let log_inner = log_block.inner(chunks[1]);
        f.render_widget(log_block, chunks[1]);
        draw_log_lines(f, log_inner, self.log.iter());

        // Keyboard with held keys lit.
        let kb_block = Block::default()
            .borders(Borders::ALL)
            .title(" keyboard (88) ");
        let kb_inner = kb_block.inner(chunks[2]);
        f.render_widget(kb_block, chunks[2]);
        let held = &self.held;
        draw_keyboard(f, kb_inner, &|note| {
            if held.is_held(note) {
                Some(HELD_COLOR)
            } else {
                None
            }
        });
    }
}

impl Default for RecordScreen {
    fn default() -> Self {
        Self::new()
    }
}
