//! Record screen: keyboard + live event log, capturing into an `EventBuffer`
//! that is saved as a bundle directory (`recordings/take-<stamp>/`).

use std::collections::VecDeque;
use std::path::PathBuf;
use std::time::Instant;

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders},
    Frame,
};
use rockcraft_audio::{play_file, BackingHandle};
use rockcraft_core::{
    BackingTrack, EventBuffer, MidiNote, NoteEvent, NoteEventKind, RecordingMeta,
};
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
    /// MIDI timestamp of recording origin (relative to this, notes become t=0
    /// for MIDI-only, or audio-start for backed recordings).
    origin_us: Option<u64>,
    /// Path of the attached backing audio file, if any.
    backing_path: Option<PathBuf>,
    /// Live playback handle for the backing track (keeps the stream alive).
    backing_handle: Option<BackingHandle>,
    /// Wall-clock instant when the backing track started playing, used to
    /// approximate origin_us in the midir timestamp domain.
    backing_start_wall: Option<Instant>,
}

impl RecordScreen {
    pub fn new() -> Self {
        Self::with_backing(None)
    }

    /// Create a record screen, optionally attaching a backing audio track.
    /// If a path is given, playback begins immediately on construction.
    pub fn with_backing(path: Option<PathBuf>) -> Self {
        let (backing_handle, backing_start_wall) = match path.as_ref() {
            Some(p) => match play_file(p) {
                Ok(h) => (Some(h), Some(Instant::now())),
                Err(e) => {
                    eprintln!("backing track start failed: {e}");
                    (None, None)
                }
            },
            None => (None, None),
        };
        let status = if backing_handle.is_some() {
            "recording + backing — [s] save  [Tab] menu".to_string()
        } else {
            "recording — [s] save  [Tab] menu".to_string()
        };
        Self {
            held: HeldNotes::new(),
            buffer: EventBuffer::new(),
            log: VecDeque::with_capacity(LOG_CAPACITY),
            status,
            origin_us: None,
            backing_path: path,
            backing_handle,
            backing_start_wall,
        }
    }

    /// Stop the backing track explicitly (also happens on drop).
    pub fn stop_backing(&self) {
        if let Some(h) = &self.backing_handle {
            h.stop();
        }
    }

    pub fn ingest(&mut self, ev: NoteEvent) {
        // Determine origin on the first event.
        // With a backing track: approximate the midir timestamp when audio
        // started (audio_start ≈ event.timestamp_us − elapsed_since_start).
        // Without: first note is t=0 (existing behaviour).
        let backing_start = self.backing_start_wall;
        let origin = *self.origin_us.get_or_insert_with(|| match backing_start {
            Some(start) => {
                let elapsed = start.elapsed().as_micros() as u64;
                ev.timestamp_us.saturating_sub(elapsed)
            }
            None => ev.timestamp_us,
        });
        let ev = rebase_event(ev, origin);
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

    /// All events captured so far, in timestamp order.
    pub fn recorded_events(&self) -> &[rockcraft_core::NoteEvent] {
        self.buffer.events()
    }

    /// Save the captured session as a bundle under `recordings/take-<stamp>/`.
    /// Returns the bundle directory path.
    pub fn save(&mut self) -> std::io::Result<PathBuf> {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let bundle_dir = std::path::Path::new(RECORDINGS_DIR).join(format!("take-{stamp}"));
        std::fs::create_dir_all(&bundle_dir)?;

        // Write MIDI
        let bytes = events_to_smf_bytes(self.buffer.events());
        std::fs::write(bundle_dir.join("song.mid"), bytes)?;

        // Optionally copy the backing track into the bundle
        let backing_meta = if let Some(ref src_path) = self.backing_path {
            let filename = bundle_backing_filename(src_path);
            std::fs::copy(src_path, bundle_dir.join(&filename))?;
            Some(BackingTrack {
                file: filename,
                audio_start_us: 0,
            })
        } else {
            None
        };

        // Write meta.json
        let meta = RecordingMeta {
            midi_file: "song.mid".to_string(),
            backing: backing_meta,
            grid: None,
            key: None,
            origin: Some(rockcraft_core::TrackOrigin::Recorded),
            video: None,
            backgrounds: Vec::new(),
            version: 1,
        };
        std::fs::write(bundle_dir.join("meta.json"), meta.to_json())?;

        self.status = format!(
            "saved {} ({} events) — [Tab] menu",
            bundle_dir.display(),
            self.buffer.len()
        );
        Ok(bundle_dir)
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

/// Rebase a single note event so its timestamp is relative to `origin_us`.
/// Pure helper — no device or I/O, testable in CI.
pub fn rebase_event(ev: NoteEvent, origin_us: u64) -> NoteEvent {
    NoteEvent {
        timestamp_us: ev.timestamp_us.saturating_sub(origin_us),
        ..ev
    }
}

/// Return the bundle-relative filename for a backing audio file.
/// Preserves the source extension; falls back to `"audio"` if absent.
pub fn bundle_backing_filename(path: &std::path::Path) -> String {
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_else(|| "audio".to_string());
    format!("backing.{ext}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{MidiNote, NoteEventKind, Velocity};

    fn ev(ts: u64) -> NoteEvent {
        NoteEvent::on(MidiNote::new(60).unwrap(), Velocity::new(80).unwrap(), ts)
    }

    // ── rebase_event ─────────────────────────────────────────────────────────

    #[test]
    fn rebase_shifts_by_origin() {
        let rebased = rebase_event(ev(1_000_000), 500_000);
        assert_eq!(rebased.timestamp_us, 500_000);
    }

    #[test]
    fn rebase_saturates_to_zero() {
        let rebased = rebase_event(ev(100), 500);
        assert_eq!(rebased.timestamp_us, 0);
    }

    #[test]
    fn rebase_preserves_note_and_kind() {
        let rebased = rebase_event(ev(1_000), 500);
        assert_eq!(rebased.note.value(), 60);
        assert!(matches!(rebased.kind, NoteEventKind::On { .. }));
    }

    #[test]
    fn rebase_origin_zero_is_identity() {
        let rebased = rebase_event(ev(42_000), 0);
        assert_eq!(rebased.timestamp_us, 42_000);
    }

    // ── bundle_backing_filename ───────────────────────────────────────────────

    #[test]
    fn backing_filename_preserves_extension() {
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("track.mp3")),
            "backing.mp3"
        );
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("/some/path/music.ogg")),
            "backing.ogg"
        );
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("file.FLAC")),
            "backing.FLAC"
        );
    }

    #[test]
    fn backing_filename_no_extension_uses_audio() {
        assert_eq!(
            bundle_backing_filename(std::path::Path::new("no_ext")),
            "backing.audio"
        );
    }

    // ── bundle meta round-trip ────────────────────────────────────────────────

    #[test]
    fn bundle_meta_roundtrip_with_backing() {
        let filename = bundle_backing_filename(std::path::Path::new("mytrack.mp3"));
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: Some(BackingTrack {
                file: filename,
                audio_start_us: 0,
            }),
            grid: None,
            key: None,
            origin: None,
            video: None,
            backgrounds: Vec::new(),
            version: 1,
        };
        let back = RecordingMeta::from_json(&meta.to_json()).unwrap();
        assert_eq!(back.midi_file, "song.mid");
        assert_eq!(back.backing.unwrap().file, "backing.mp3");
        assert_eq!(back.version, 1);
    }

    #[test]
    fn bundle_meta_roundtrip_midi_only() {
        let meta = RecordingMeta::new_midi_only("song.mid");
        let back = RecordingMeta::from_json(&meta.to_json()).unwrap();
        assert_eq!(back.midi_file, "song.mid");
        assert!(back.backing.is_none());
    }
}
