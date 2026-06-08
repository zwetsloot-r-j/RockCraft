//! Play screen: a falling-note highway above the keyboard. Loads a `.mid`,
//! scrolls its notes down to the keyboard line on a playback clock, and lights
//! the player's live keys over it (play-along; scoring is a later task).

use std::collections::{BTreeSet, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};
use rockcraft_audio::{play_file_at, BackingHandle, SynthHandle};
use rockcraft_core::{
    backing_position_us, GateState, MidiNote, NoteEvent, PlayClock, Velocity, WaitGate,
};
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

/// Velocity used when the hear-the-song feature synthesizes recorded notes.
const HEAR_VELOCITY: u8 = 80;

/// A backing audio track attached to a bundle, plus the file position that
/// lines up with recording time 0 (`audio_start_us`, from Task C).
struct Backing {
    path: PathBuf,
    audio_start_us: u64,
}

pub struct PlayScreen {
    spans: Vec<NoteSpan>,
    duration_us: u64,
    held: HeldNotes,
    /// Pausable song-time clock (M5-A). Replaces the old free-running `Instant`
    /// so wait-mode can freeze the highway and the backing in lock-step. It only
    /// accrues while running; `tick` advances it by the wall-clock frame delta.
    clock: PlayClock,
    /// Wall-clock anchor for the last `tick`; `None` until the first tick (and
    /// after `restart`), so the first delta is zero. This is the only `Instant`
    /// the screen keeps — purely to measure real elapsed time between frames.
    last_tick: Option<Instant>,
    /// Note-by-note wait gate (M5-A). Disarmed = free play-through (today's
    /// behaviour); armed = freeze `clock` + backing on an unsatisfied due step.
    wait: WaitGate,
    title: String,
    finished_pause_us: u64,
    synth: Option<SynthHandle>,
    /// Whole-song forward shift applied to the spans; the clock value at which
    /// the first note's lead-in ends and the backing track should begin.
    /// Equals `song_shift_us(first_note_us, PRE_ROLL_US, LEAD_US)`.
    shift_us: u64,
    /// Backing track to sync, if the loaded bundle has one.
    backing: Option<Backing>,
    /// Live playback handle once the backing track has started; `None` until the
    /// clock reaches `shift_us` (and again after `restart` re-arms it).
    backing_handle: Option<BackingHandle>,
    /// Whether the "hear the song" feature is active.
    hear_song: bool,
    /// Span indices for which we have already sent note_on to the song synth.
    song_on_fired: HashSet<usize>,
    /// Span indices for which we have already sent note_off to the song synth.
    song_off_fired: HashSet<usize>,
}

impl PlayScreen {
    /// Load a song from `.mid` bytes.
    pub fn from_smf_bytes(
        title: String,
        bytes: &[u8],
        synth: Option<SynthHandle>,
    ) -> Result<Self, String> {
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
        let wait = WaitGate::from_expected(&expected_steps(&spans));

        Ok(Self {
            spans,
            duration_us,
            held: HeldNotes::new(),
            clock: PlayClock::new(),
            last_tick: None,
            wait,
            title,
            finished_pause_us: LEAD_US,
            synth,
            shift_us: offset,
            backing: None,
            backing_handle: None,
            hear_song: false,
            song_on_fired: HashSet::new(),
            song_off_fired: HashSet::new(),
        })
    }

    /// Attach a backing audio track from the loaded bundle. `audio_start_us` is
    /// the position in the file that lines up with recording time 0 (Task C).
    /// Playback is armed lazily: it begins when the clock reaches `shift_us`.
    pub fn with_backing(mut self, path: PathBuf, audio_start_us: u64) -> Self {
        self.backing = Some(Backing {
            path,
            audio_start_us,
        });
        self
    }

    /// Restart playback from the top; resets synth state and re-arms the backing
    /// track so it re-syncs from `shift_us` again.
    pub fn restart(&mut self) {
        self.clock.reset();
        self.last_tick = None;
        // Rebuild the wait tracker from the top, keeping wait-mode armed or not.
        let armed = self.wait.is_armed();
        self.wait = WaitGate::from_expected(&expected_steps(&self.spans));
        self.wait.set_armed(armed);
        self.song_on_fired.clear();
        self.song_off_fired.clear();
        if let Some(s) = &self.synth {
            s.all_off();
        }
        if let Some(h) = self.backing_handle.take() {
            h.stop();
        }
    }

    /// The file position the backing track should be at for clock `now_us`, or
    /// `None` if it should not be playing yet (clock still in the lead-in) or
    /// there is no backing track. Shares core's `backing_position_us` formula so
    /// the audio and the highway never drift apart.
    fn backing_target_us(&self, now_us: u64) -> Option<u64> {
        let b = self.backing.as_ref()?;
        backing_position_us(now_us, self.shift_us, b.audio_start_us)
    }

    /// Start the backing track once the clock first reaches `shift_us`, seeking
    /// the file to the matching position. Call once per event-loop iteration
    /// (clock-driven, like [`Self::tick_song_synth`]). A no-op when there is no
    /// backing track or it is already playing. Never blocks the audio thread.
    pub fn tick_backing(&mut self) {
        if self.backing_handle.is_some() {
            return;
        }
        let Some(pos_us) = self.backing_target_us(self.now_us()) else {
            return;
        };
        let Some(b) = &self.backing else {
            return;
        };
        match play_file_at(&b.path, Duration::from_micros(pos_us)) {
            Ok(h) => {
                // If the highway is frozen by wait-mode at the moment the
                // backing arms, start it paused so it never gets ahead.
                if !self.clock.is_running() {
                    h.pause();
                }
                self.backing_handle = Some(h);
            }
            // On a persistent failure (missing/undecodable file), drop the track
            // rather than retry every frame; the song still plays silently.
            Err(_) => self.backing = None,
        }
    }

    /// Advance the playback clock by the wall-clock time elapsed since the last
    /// tick, gated by wait-mode. Called once per run-loop iteration; this is the
    /// frontend clock the pure seams omit. Mirrors `EditScreen::tick_audition`.
    pub fn tick(&mut self) {
        let now = Instant::now();
        let dt = self
            .last_tick
            .map(|prev| now.duration_since(prev).as_micros() as u64)
            .unwrap_or(0);
        self.last_tick = Some(now);
        self.advance(dt);
    }

    /// Advance the gated clock by `dt_us`. The headless seam wait/clock tests
    /// drive directly: it feeds the live held notes into the [`WaitGate`], polls
    /// it at the current clock position, freezes (`clock` + backing paused) or
    /// resumes on the transition, then advances the clock by `dt_us` — a no-op
    /// while frozen. With wait-mode disarmed the gate is always `Running`, so
    /// this is exactly the old free-running advance (no regression).
    pub fn advance(&mut self, dt_us: u64) {
        let held: BTreeSet<u8> = self.held.iter().collect();
        self.wait.set_held(held);
        let frozen = self.wait.poll(self.clock.now_us()) == GateState::Frozen;
        // Only act on the running↔frozen transition so we don't spam the audio
        // thread with pause/resume commands every frame.
        if frozen && self.clock.is_running() {
            self.clock.pause();
            if let Some(h) = &self.backing_handle {
                h.pause();
            }
        } else if !frozen && !self.clock.is_running() {
            self.clock.resume();
            if let Some(h) = &self.backing_handle {
                h.resume();
            }
        }
        self.clock.advance(dt_us);
    }

    /// Toggle note-by-note wait-mode (the `w` key / `Action::ToggleWaitMode`).
    pub fn toggle_wait_mode(&mut self) {
        self.set_wait_mode(!self.wait.is_armed());
    }

    /// Set wait-mode on/off (`Action::SetWaitMode`). Turning it off immediately
    /// un-freezes the clock and backing so play resumes without waiting a frame.
    pub fn set_wait_mode(&mut self, on: bool) {
        self.wait.set_armed(on);
        if !on && !self.clock.is_running() {
            self.clock.resume();
            if let Some(h) = &self.backing_handle {
                h.resume();
            }
        }
    }

    /// Is wait-mode armed? (For the status line / control snapshot.)
    pub fn is_wait_mode(&self) -> bool {
        self.wait.is_armed()
    }

    /// The notes the player must hold to un-freeze, if currently waiting.
    fn awaiting_notes(&self) -> Option<Vec<u8>> {
        self.wait.awaiting().map(|step| step.notes.clone())
    }

    /// Whether the backing track is currently audible (for the status line).
    fn backing_playing(&self) -> bool {
        self.backing_handle.is_some()
    }

    /// Forward a live `NoteEvent` to both the held-key tracker and the synth.
    pub fn ingest(&mut self, ev: NoteEvent) {
        self.held.apply(&ev);
        if let Some(s) = &self.synth {
            s.apply(&ev);
        }
    }

    /// Toggle the "hear the song" feature. Turning it off silences any playing
    /// song notes and resets the trigger bookkeeping.
    pub fn toggle_hear_song(&mut self) {
        self.hear_song = !self.hear_song;
        if !self.hear_song {
            self.song_on_fired.clear();
            self.song_off_fired.clear();
            if let Some(s) = &self.synth {
                s.all_off();
            }
        }
    }

    /// Check the playback clock and fire synth note_on / note_off commands for
    /// any song spans whose boundaries we've crossed since the last call.
    /// Call this once per event-loop iteration (not per render frame) to keep
    /// audio timing driven by the clock rather than the frame rate.
    pub fn tick_song_synth(&mut self) {
        if !self.hear_song {
            return;
        }
        let now = self.now_us();
        let (need_on, need_off) =
            pending_triggers(&self.spans, now, &self.song_on_fired, &self.song_off_fired);
        let velocity = Velocity::new(HEAR_VELOCITY).unwrap();
        for i in need_on {
            if let Some(note) = MidiNote::new(self.spans[i].note) {
                if let Some(s) = &self.synth {
                    s.note_on(note, velocity);
                }
            }
            self.song_on_fired.insert(i);
        }
        for i in need_off {
            if let Some(note) = MidiNote::new(self.spans[i].note) {
                if let Some(s) = &self.synth {
                    s.note_off(note);
                }
            }
            self.song_off_fired.insert(i);
        }
    }

    /// Silence all notes and stop the backing track — call when leaving the
    /// screen. (The handle also stops when the `PlayScreen` is dropped.)
    pub fn leave(&self) {
        if let Some(s) = &self.synth {
            s.all_off();
        }
        if let Some(h) = &self.backing_handle {
            h.stop();
        }
    }

    /// Current playback time in microseconds since entering the screen. Reads
    /// the pausable [`PlayClock`]; frozen wait-mode holds this value steady.
    fn now_us(&self) -> u64 {
        self.clock.now_us()
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
        let music_color = if self.hear_song {
            Color::Green
        } else {
            Color::DarkGray
        };
        let wait_color = if self.is_wait_mode() {
            Color::Green
        } else {
            Color::DarkGray
        };
        let mut spans = vec![
            Span::styled(" PLAY ", Style::default().fg(Color::Black).bg(TARGET_COLOR)),
            Span::raw(format!("  {:.1}s / {:.1}s  ", secs, total)),
            Span::raw("[r] restart  [Tab] menu  "),
            Span::styled("[m] music  ", Style::default().fg(music_color)),
            Span::styled("[w] wait  ", Style::default().fg(wait_color)),
        ];
        if self.backing_playing() {
            spans.push(Span::styled(
                "♪ backing  ",
                Style::default().fg(Color::Green),
            ));
        }
        // While frozen, tell the player exactly what to hold to continue.
        if let Some(notes) = self.awaiting_notes() {
            let names = notes
                .iter()
                .filter_map(|&p| MidiNote::new(p).map(|n| n.name()))
                .collect::<Vec<_>>()
                .join(" ");
            spans.push(Span::styled(
                format!("⏸ waiting — play {names}  "),
                Style::default().fg(Color::Yellow),
            ));
        }
        spans.extend([
            Span::styled("● target ", Style::default().fg(TARGET_COLOR)),
            Span::styled("● you ", Style::default().fg(HELD_COLOR)),
            Span::styled("● match", Style::default().fg(MATCH_COLOR)),
        ]);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
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

/// Expected `(pitch, start_us)` pairs feeding the [`WaitGate`]: every span's
/// note at its (already-shifted) start. Notes sharing a start collapse into one
/// chord step inside the gate.
fn expected_steps(spans: &[NoteSpan]) -> Vec<(MidiNote, u64)> {
    spans
        .iter()
        .filter_map(|s| MidiNote::new(s.note).map(|n| (n, s.start_us)))
        .collect()
}

/// Returns `(need_on, need_off)`: indices into `spans` where note_on / note_off
/// should fire at `now_us` but haven't yet. Pure; suitable for unit testing.
fn pending_triggers(
    spans: &[NoteSpan],
    now_us: u64,
    on_fired: &HashSet<usize>,
    off_fired: &HashSet<usize>,
) -> (Vec<usize>, Vec<usize>) {
    let mut need_on = Vec::new();
    let mut need_off = Vec::new();
    for (i, span) in spans.iter().enumerate() {
        if now_us >= span.start_us && !on_fired.contains(&i) {
            need_on.push(i);
        }
        if now_us >= span.end_us && !off_fired.contains(&i) {
            need_off.push(i);
        }
    }
    (need_on, need_off)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn span(start_us: u64, end_us: u64) -> NoteSpan {
        NoteSpan {
            note: 60,
            start_us,
            end_us,
        }
    }

    fn empty() -> HashSet<usize> {
        HashSet::new()
    }

    #[test]
    fn no_triggers_before_any_span_starts() {
        let spans = vec![span(1000, 2000), span(3000, 4000)];
        let (on, off) = pending_triggers(&spans, 500, &empty(), &empty());
        assert!(on.is_empty());
        assert!(off.is_empty());
    }

    #[test]
    fn note_on_fires_when_clock_reaches_start() {
        let spans = vec![span(1000, 2000)];
        let (on, off) = pending_triggers(&spans, 1000, &empty(), &empty());
        assert_eq!(on, vec![0]);
        assert!(off.is_empty());
    }

    #[test]
    fn note_off_fires_when_clock_reaches_end() {
        let spans = vec![span(1000, 2000)];
        let mut on_fired = HashSet::new();
        on_fired.insert(0);
        let (on, off) = pending_triggers(&spans, 2000, &on_fired, &empty());
        assert!(on.is_empty(), "on should not fire twice");
        assert_eq!(off, vec![0]);
    }

    #[test]
    fn each_fires_exactly_once() {
        let spans = vec![span(1000, 2000)];
        let mut on_fired = HashSet::new();
        let mut off_fired = HashSet::new();

        // Before start: nothing
        let (on, off) = pending_triggers(&spans, 999, &on_fired, &off_fired);
        assert!(on.is_empty() && off.is_empty());

        // At start: note_on
        let (on, off) = pending_triggers(&spans, 1000, &on_fired, &off_fired);
        assert_eq!(on, vec![0]);
        assert!(off.is_empty());
        on_fired.insert(0);

        // Between start and end: nothing new
        let (on, off) = pending_triggers(&spans, 1500, &on_fired, &off_fired);
        assert!(on.is_empty() && off.is_empty());

        // At end: note_off
        let (on, off) = pending_triggers(&spans, 2000, &on_fired, &off_fired);
        assert!(on.is_empty());
        assert_eq!(off, vec![0]);
        off_fired.insert(0);

        // After end: nothing new
        let (on, off) = pending_triggers(&spans, 3000, &on_fired, &off_fired);
        assert!(on.is_empty() && off.is_empty());
    }

    #[test]
    fn multiple_spans_fire_independently() {
        let spans = vec![span(1000, 2000), span(1500, 3000)];
        let (on, off) = pending_triggers(&spans, 1500, &empty(), &empty());
        // Both have started; neither has ended yet.
        assert_eq!(on, vec![0, 1]);
        assert!(off.is_empty());
    }

    // ── backing-track sync decision (shares core's backing_position_us) ───────

    use rockcraft_midi::events_to_smf_bytes;

    /// A one-note song whose single note is at t=0, so the whole-song shift is
    /// `PRE_ROLL_US + LEAD_US` (no first-note compensation).
    fn one_note_screen() -> PlayScreen {
        let events = vec![
            NoteEvent::on(MidiNote::new(60).unwrap(), Velocity::new(80).unwrap(), 0),
            NoteEvent::off(MidiNote::new(60).unwrap(), 500_000),
        ];
        let bytes = events_to_smf_bytes(&events);
        PlayScreen::from_smf_bytes("test".into(), &bytes, None).unwrap()
    }

    #[test]
    fn no_backing_track_never_targets() {
        let play = one_note_screen();
        assert_eq!(play.backing_target_us(0), None);
        assert_eq!(play.backing_target_us(10_000_000), None);
    }

    #[test]
    fn backing_silent_during_lead_in() {
        let play = one_note_screen().with_backing(PathBuf::from("backing.mp3"), 0);
        let shift = PRE_ROLL_US + LEAD_US;
        // Before the lead-in ends: no audio yet.
        assert_eq!(play.backing_target_us(0), None);
        assert_eq!(play.backing_target_us(shift - 1), None);
    }

    #[test]
    fn backing_starts_at_shift_and_tracks_clock() {
        let play = one_note_screen().with_backing(PathBuf::from("backing.mp3"), 0);
        let shift = PRE_ROLL_US + LEAD_US;
        // At the shift point the file plays from its start.
        assert_eq!(play.backing_target_us(shift), Some(0));
        // One second later, one second into the file.
        assert_eq!(play.backing_target_us(shift + 1_000_000), Some(1_000_000));
    }

    #[test]
    fn backing_respects_audio_start_offset() {
        let play = one_note_screen().with_backing(PathBuf::from("backing.mp3"), 250_000);
        let shift = PRE_ROLL_US + LEAD_US;
        // A trimmed lead-in: at the shift point the file is already 250ms in.
        assert_eq!(play.backing_target_us(shift), Some(250_000));
        assert_eq!(play.backing_target_us(shift + 1_000_000), Some(1_250_000));
    }

    #[test]
    fn restart_clears_state_so_triggers_refire() {
        let spans = vec![span(1000, 2000)];
        let mut on_fired = HashSet::new();
        let mut off_fired = HashSet::new();
        on_fired.insert(0);
        off_fired.insert(0);

        // With fired state present, nothing fires again.
        let (on, off) = pending_triggers(&spans, 2000, &on_fired, &off_fired);
        assert!(on.is_empty() && off.is_empty());

        // After clearing (simulating restart), both fire again.
        on_fired.clear();
        off_fired.clear();
        let (on, off) = pending_triggers(&spans, 2000, &on_fired, &off_fired);
        assert_eq!(on, vec![0]);
        assert_eq!(off, vec![0]);
    }

    // ── wait-mode (freeze highway + backing until the right notes are held) ──

    const SHIFT: u64 = PRE_ROLL_US + LEAD_US;

    fn note_on(play: &mut PlayScreen, pitch: u8) {
        play.ingest(NoteEvent::on(
            MidiNote::new(pitch).unwrap(),
            Velocity::new(80).unwrap(),
            0,
        ));
    }

    fn note_off(play: &mut PlayScreen, pitch: u8) {
        play.ingest(NoteEvent::off(MidiNote::new(pitch).unwrap(), 0));
    }

    /// C at t=0 then D at t=1s. After the whole-song shift the steps fall at
    /// `SHIFT` (C) and `SHIFT + 1_000_000` (D).
    fn two_note_screen() -> PlayScreen {
        let c = MidiNote::new(60).unwrap();
        let d = MidiNote::new(62).unwrap();
        let v = Velocity::new(80).unwrap();
        let events = vec![
            NoteEvent::on(c, v, 0),
            NoteEvent::off(c, 500_000),
            NoteEvent::on(d, v, 1_000_000),
            NoteEvent::off(d, 1_500_000),
        ];
        let bytes = events_to_smf_bytes(&events);
        PlayScreen::from_smf_bytes("test".into(), &bytes, None).unwrap()
    }

    #[test]
    fn armed_clock_freezes_on_unsatisfied_step_and_resumes_when_held() {
        let mut play = two_note_screen();
        play.set_wait_mode(true);
        // Run the lead-in up to the first step's time.
        play.advance(SHIFT);
        assert_eq!(play.now_us(), SHIFT);
        // Nothing held: the step is due and unsatisfied, so the clock freezes —
        // a big delta does NOT move the playhead past the step.
        play.advance(5_000_000);
        play.advance(5_000_000);
        assert_eq!(
            play.now_us(),
            SHIFT,
            "clock must freeze on the awaited step"
        );
        // Hold the required C: the gate advances and the clock resumes.
        note_on(&mut play, 60);
        play.advance(1_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000, "clock advances once held");
    }

    #[test]
    fn chord_step_requires_all_notes_extras_allowed() {
        let c = MidiNote::new(60).unwrap();
        let e = MidiNote::new(64).unwrap();
        let g = MidiNote::new(67).unwrap();
        let v = Velocity::new(80).unwrap();
        let events = vec![
            NoteEvent::on(c, v, 0),
            NoteEvent::on(e, v, 0),
            NoteEvent::on(g, v, 0),
            NoteEvent::off(c, 500_000),
            NoteEvent::off(e, 500_000),
            NoteEvent::off(g, 500_000),
        ];
        let bytes = events_to_smf_bytes(&events);
        let mut play = PlayScreen::from_smf_bytes("chord".into(), &bytes, None).unwrap();
        play.set_wait_mode(true);
        play.advance(SHIFT);
        // Partial chord (missing G) keeps it frozen.
        note_on(&mut play, 60);
        note_on(&mut play, 64);
        play.advance(2_000_000);
        assert_eq!(
            play.now_us(),
            SHIFT,
            "partial chord must not satisfy the step"
        );
        // Full chord plus an extra note (allowed) releases the freeze.
        note_on(&mut play, 67);
        note_on(&mut play, 72);
        play.advance(1_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
    }

    #[test]
    fn disarmed_wait_mode_advances_freely() {
        let mut play = two_note_screen();
        // Wait-mode off (default): nothing held, yet the playhead runs straight
        // through both steps — today's free play-through, no regression.
        assert!(!play.is_wait_mode());
        play.advance(SHIFT + 5_000_000);
        assert_eq!(play.now_us(), SHIFT + 5_000_000);
    }

    #[test]
    fn toggling_wait_off_resumes_a_frozen_clock() {
        let mut play = two_note_screen();
        play.toggle_wait_mode();
        assert!(play.is_wait_mode());
        play.advance(SHIFT);
        play.advance(1_000_000); // freezes at SHIFT (nothing held)
        assert_eq!(play.now_us(), SHIFT);
        // Turning wait-mode off must unfreeze immediately.
        play.toggle_wait_mode();
        assert!(!play.is_wait_mode());
        play.advance(1_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
    }

    #[test]
    fn releasing_a_held_note_re_freezes_on_the_next_step() {
        let mut play = two_note_screen();
        play.set_wait_mode(true);
        play.advance(SHIFT);
        // Satisfy the C step; clock advances toward the D step.
        note_on(&mut play, 60);
        play.advance(1_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
        // Release everything: the D step is now due and unsatisfied → frozen.
        note_off(&mut play, 60);
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000, "re-freezes on the D step");
        // Holding D resumes again.
        note_on(&mut play, 62);
        play.advance(500_000);
        assert_eq!(play.now_us(), SHIFT + 1_500_000);
    }

    #[test]
    fn frozen_backing_target_does_not_drift() {
        let mut play = two_note_screen().with_backing(PathBuf::from("backing.mp3"), 0);
        play.set_wait_mode(true);
        play.advance(SHIFT);
        // The backing position the highway expects at the freeze point.
        let before = play.backing_target_us(play.now_us());
        assert_eq!(before, Some(0));
        // While frozen (nothing held) the clock — and thus the backing target —
        // must not move, so audio and highway resume in sync.
        play.advance(10_000_000);
        assert_eq!(play.now_us(), SHIFT);
        assert_eq!(play.backing_target_us(play.now_us()), before, "no drift");
    }

    #[test]
    fn restart_rearms_wait_tracker_from_the_top() {
        let mut play = two_note_screen();
        play.set_wait_mode(true);
        note_on(&mut play, 60);
        play.advance(SHIFT);
        play.advance(1_000_000); // past the C step
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
        // Restart resets the clock and the tracker; wait-mode stays armed and the
        // C step must be awaited again from t=0.
        play.restart();
        assert!(play.is_wait_mode());
        assert_eq!(play.now_us(), 0);
        note_off(&mut play, 60);
        play.advance(SHIFT);
        play.advance(1_000_000);
        assert_eq!(
            play.now_us(),
            SHIFT,
            "C step is awaited again after restart"
        );
    }
}
