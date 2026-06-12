//! Play-session engine + IPC for the Tauri Highway screen (#168).
//!
//! Replaces the webview's Ember Lantern mock and its `performance.now()` clock
//! with a real session at parity with the TUI play screen
//! (`crates/tui/src/play.rs`): a bundle of [`NoteSpan`]s driven by a pausable
//! [`PlayClock`], gated by a [`WaitGate`] (wait mode), with live scoring against
//! [`rockcraft_core::scoring`] and an end-of-take [`PlaySummary`] from
//! [`rockcraft_core::stats`].
//!
//! CLAUDE.md invariants honoured here:
//! - **No wall-clock inside the engine.** Timing is injected via
//!   [`PlaySession::advance`] (`dt_us`); the wall clock lives only in the
//!   `lib.rs` tick thread, exactly as `play.rs::tick()` measures the frame delta
//!   there.
//! - **Scoring keys off MIDI timestamps**, never the render loop: played
//!   note-ons are collected with their `timestamp_us` and scored with
//!   [`rockcraft_core::score`].
//! - **Frontend is swappable.** [`PlaySession`] is pure state + plain
//!   serializable reports; synth / backing / event-emit side effects are applied
//!   by [`tick_play`] / the command wrappers, mirroring the TUI's
//!   `tick_song_synth` / `tick_backing`.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use rockcraft_core::{
    backing_position_us, score, song_shift_us, ExpectedNote, GateState, MidiNote, NoteEvent,
    NoteEventKind, NoteJudgment, PlayClock, RecordingMeta, ScoreConfig, ScoreReport, Summary,
    Timing, WaitGate,
};
use rockcraft_midi::smf_bytes_to_events;
use serde::Serialize;

use crate::audio::AudioState;

/// How far into the future the top of the highway represents (microseconds).
/// Matches the TUI `play.rs` `LEAD_US` so a bundle scrolls identically.
pub const LEAD_US: u64 = 2_000_000;

/// Empty pre-roll before the first note enters the top of the highway. Total
/// lead-in before the first note reaches the keyboard is `PRE_ROLL_US +
/// LEAD_US`. Matches the TUI `play.rs` `PRE_ROLL_US`.
pub const PRE_ROLL_US: u64 = 1_500_000;

/// Velocity used when "hear the song" synthesises chart notes. Matches the TUI.
pub const HEAR_VELOCITY: u8 = 80;

/// A sustained note: pitch held from `start_us` to `end_us` (microseconds,
/// already shifted by the pre-roll). Mirrors `tui::highway::NoteSpan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteSpan {
    pub note: u8,
    pub start_us: u64,
    pub end_us: u64,
}

/// A backing audio track attached to a bundle, plus the file position that lines
/// up with song time 0 (`audio_start_us`).
#[derive(Debug, Clone)]
pub struct Backing {
    pub path: PathBuf,
    pub audio_start_us: u64,
}

/// Static song info returned by `play_load` so the webview can configure the
/// highway (title, spans, lead-in shift, total length, backing presence).
#[derive(Debug, Clone, Serialize)]
pub struct PlayInfo {
    pub title: String,
    /// Spans in milliseconds (the webview engine is ms-based), already shifted.
    pub notes: Vec<SpanView>,
    /// Whole-song forward shift in microseconds (`song_shift_us`).
    pub shift_us: u64,
    /// Total song length including the lead-in, in microseconds.
    pub duration_us: u64,
    /// Lead window the highway shows top→hit-line, in microseconds.
    pub lead_us: u64,
    /// Whether a backing track is attached.
    pub has_backing: bool,
    /// Whether "hear the song" starts on.
    pub hear_song: bool,
}

/// One span projected for the webview: pitch + ms bounds. No hand info exists in
/// a bundle, so — like the TUI — every note shares one color mode (the webview's
/// spectrum-by-pitch handles coloring); `hand` is the frontend default.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SpanView {
    pub note: u8,
    /// Start in milliseconds.
    pub start: f64,
    /// End in milliseconds.
    pub end: f64,
}

/// A ~60 Hz live snapshot pushed to the webview while a take is running.
#[derive(Debug, Clone, Serialize)]
pub struct PlayStateEvent {
    pub time_us: u64,
    pub frozen: bool,
    pub score: u64,
    pub combo: u32,
    pub best_combo: u32,
    pub hits: usize,
    pub misses: usize,
    /// Notes currently held by the player.
    pub held: Vec<u8>,
    /// Notes the player must hold to un-freeze (empty unless `frozen`).
    pub awaiting: Vec<u8>,
    /// Set once the song (plus tail) has finished.
    pub finished: bool,
}

/// The end-of-take summary returned by `play_finish`.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct PlaySummary {
    pub total_expected: usize,
    pub hits: usize,
    pub misses: usize,
    pub extras: usize,
    pub perfect: usize,
    pub early: usize,
    pub late: usize,
    /// Accuracy in basis points (0..=10000) so it is JSON-exact; divide by 100
    /// for a percentage. Mirrors `core::stats::Summary::accuracy`.
    pub accuracy_bp: u32,
    pub best_combo: u32,
    pub score: u64,
}

/// Build sustained spans from a time-ordered event stream by pairing each
/// note-on with the next note-off (or note-on vel 0) of the same pitch. A local
/// copy of `tui::highway::build_spans` (the tauri crate cannot depend on tui).
fn build_spans(events: &[NoteEvent]) -> Vec<NoteSpan> {
    let mut open: std::collections::HashMap<u8, u64> = std::collections::HashMap::new();
    let mut spans = Vec::new();
    let mut last_us = 0u64;

    for ev in events {
        last_us = last_us.max(ev.timestamp_us);
        let pitch = ev.note.value();
        match ev.kind {
            NoteEventKind::On { velocity } if velocity.value() > 0 => {
                if let Some(start) = open.remove(&pitch) {
                    spans.push(NoteSpan {
                        note: pitch,
                        start_us: start,
                        end_us: ev.timestamp_us,
                    });
                }
                open.insert(pitch, ev.timestamp_us);
            }
            _ => {
                if let Some(start) = open.remove(&pitch) {
                    spans.push(NoteSpan {
                        note: pitch,
                        start_us: start,
                        end_us: ev.timestamp_us,
                    });
                }
            }
        }
    }

    for (pitch, start) in open {
        spans.push(NoteSpan {
            note: pitch,
            start_us: start,
            end_us: last_us.max(start + 1),
        });
    }

    spans.sort_by_key(|s| (s.start_us, s.note));
    spans
}

/// Total song length = the latest end across all spans (0 if empty).
fn song_duration_us(spans: &[NoteSpan]) -> u64 {
    spans.iter().map(|s| s.end_us).max().unwrap_or(0)
}

/// Expected `(pitch, start_us)` pairs for the [`WaitGate`]: every span's note at
/// its (already-shifted) start. Notes sharing a start collapse into one chord
/// step inside the gate. Mirrors `play.rs::expected_steps`.
fn expected_steps(spans: &[NoteSpan]) -> Vec<(MidiNote, u64)> {
    spans
        .iter()
        .filter_map(|s| MidiNote::new(s.note).map(|n| (n, s.start_us)))
        .collect()
}

/// One scoring target per span (pitch at its shifted start). Held separately
/// from the wait steps so a chord scores as N independent expected notes.
fn expected_notes(spans: &[NoteSpan]) -> Vec<ExpectedNote> {
    spans
        .iter()
        .filter_map(|s| MidiNote::new(s.note).map(|n| ExpectedNote::new(n, s.start_us)))
        .collect()
}

/// Per-note score award. Perfect = 100 + combo·2, Good (early/late) = 50 +
/// combo. A miss zeroes the combo. Mirrors the webview prototype's economy so
/// the header reads the same range as the old mock, but driven by real timing.
fn award(perfect: bool, combo: u32) -> u64 {
    if perfect {
        100 + combo as u64 * 2
    } else {
        50 + combo as u64
    }
}

/// A live play session: bundle spans driven by an injected clock, gated by a
/// wait gate, with incremental scoring. Pure state — no device, no wall clock.
pub struct PlaySession {
    spans: Vec<NoteSpan>,
    title: String,
    duration_us: u64,
    finished_pause_us: u64,
    clock: PlayClock,
    wait: WaitGate,
    /// Whole-song forward shift (`song_shift_us`); the clock value at which the
    /// backing track begins.
    shift_us: u64,
    backing: Option<Backing>,
    hear_song: bool,
    cfg: ScoreConfig,

    /// Live held-note set, updated by every ingested MIDI event.
    held: BTreeSet<u8>,
    /// Every player note-on collected with its `timestamp_us` for final scoring.
    played: Vec<NoteEvent>,

    /// Per-span incremental judgement: a span is "scored" once the clock passes
    /// its good-window close, so the header combo/score is live. Indices into
    /// `spans`.
    scored: HashSet<usize>,
    score: u64,
    combo: u32,
    best_combo: u32,
    live_hits: usize,
    live_misses: usize,

    /// "Hear the song" audition trigger bookkeeping (span indices fired).
    song_on_fired: HashSet<usize>,
    song_off_fired: HashSet<usize>,
}

impl PlaySession {
    /// Load a song from `.mid` bytes, applying the whole-song pre-roll shift so
    /// the highway opens empty and the first note reaches the keyboard after
    /// `PRE_ROLL_US + LEAD_US`. Mirrors `PlayScreen::from_smf_bytes`.
    pub fn from_smf_bytes(title: String, bytes: &[u8]) -> Result<Self, String> {
        let events = smf_bytes_to_events(bytes).map_err(|e| e.to_string())?;
        Ok(Self::from_events(title, &events))
    }

    /// Build a session directly from a note-event timeline (the SMF parse seam,
    /// exposed for headless tests with `ScriptedSource`-style fixtures).
    pub fn from_events(title: String, events: &[NoteEvent]) -> Self {
        let raw = build_spans(events);
        let first_us = raw.iter().map(|s| s.start_us).min().unwrap_or(0);
        let shift_us = song_shift_us(first_us, PRE_ROLL_US, LEAD_US);
        let spans: Vec<NoteSpan> = raw
            .into_iter()
            .map(|s| NoteSpan {
                note: s.note,
                start_us: s.start_us + shift_us,
                end_us: s.end_us + shift_us,
            })
            .collect();
        let duration_us = song_duration_us(&spans);
        let wait = WaitGate::from_expected(&expected_steps(&spans));

        Self {
            spans,
            title,
            duration_us,
            finished_pause_us: LEAD_US,
            clock: PlayClock::new(),
            wait,
            shift_us,
            backing: None,
            hear_song: false,
            cfg: ScoreConfig::default(),
            held: BTreeSet::new(),
            played: Vec::new(),
            scored: HashSet::new(),
            score: 0,
            combo: 0,
            best_combo: 0,
            live_hits: 0,
            live_misses: 0,
            song_on_fired: HashSet::new(),
            song_off_fired: HashSet::new(),
        }
    }

    /// Attach a backing track resolved from the bundle's `meta.json`.
    pub fn with_backing(mut self, path: PathBuf, audio_start_us: u64) -> Self {
        self.backing = Some(Backing {
            path,
            audio_start_us,
        });
        self
    }

    /// Start with "hear the song" on (imported charts opt in; play-along leaves
    /// it off so the song doesn't sound over the player). Test-only for now —
    /// the live path leaves it off and toggles via `play_toggle_hear_song`.
    #[cfg(test)]
    pub fn with_hear_song(mut self, on: bool) -> Self {
        self.hear_song = on;
        self
    }

    /// The static info payload for `play_load`.
    pub fn info(&self) -> PlayInfo {
        PlayInfo {
            title: self.title.clone(),
            notes: self
                .spans
                .iter()
                .map(|s| SpanView {
                    note: s.note,
                    start: s.start_us as f64 / 1000.0,
                    end: s.end_us as f64 / 1000.0,
                })
                .collect(),
            shift_us: self.shift_us,
            duration_us: self.duration_us,
            lead_us: LEAD_US,
            has_backing: self.backing.is_some(),
            hear_song: self.hear_song,
        }
    }

    /// Current song time in microseconds (reads the pausable clock).
    pub fn now_us(&self) -> u64 {
        self.clock.now_us()
    }

    /// Whole-song shift (where the backing begins). Test-only assertion helper;
    /// the live path reads it through `info().shift_us`.
    #[cfg(test)]
    pub fn shift_us(&self) -> u64 {
        self.shift_us
    }

    /// Is wait-mode armed?
    pub fn is_wait_mode(&self) -> bool {
        self.wait.is_armed()
    }

    /// The backing track, if any.
    pub fn backing(&self) -> Option<&Backing> {
        self.backing.as_ref()
    }

    /// Has the song (plus tail) finished?
    pub fn is_finished(&self) -> bool {
        self.now_us() > self.duration_us + self.finished_pause_us
    }

    /// Forward a live `NoteEvent`: update the held set and (on a real strike)
    /// collect it for scoring with its MIDI timestamp.
    pub fn ingest(&mut self, ev: NoteEvent) {
        match ev.kind {
            NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                self.held.insert(ev.note.value());
                self.played.push(ev);
            }
            _ => {
                self.held.remove(&ev.note.value());
            }
        }
    }

    /// Advance the gated clock by `dt_us`, exactly like `play.rs::advance`: feed
    /// the held set into the wait gate, freeze/resume on the transition, then
    /// advance (a no-op while frozen). After advancing, fold any spans whose
    /// good-window has now closed into the live score. Returns whether the clock
    /// is frozen after this step (so the caller can pause the backing).
    pub fn advance(&mut self, dt_us: u64) -> bool {
        self.wait.set_held(self.held.clone());
        let frozen = self.wait.poll(self.clock.now_us()) == GateState::Frozen;
        if frozen && self.clock.is_running() {
            self.clock.pause();
        } else if !frozen && !self.clock.is_running() {
            self.clock.resume();
        }
        self.clock.advance(dt_us);
        self.score_due();
        frozen
    }

    /// Score every span whose good-window has closed since the last call. A span
    /// at shifted start `t` is final once the clock passes `t + good_us`: by
    /// then any in-window strike has arrived. We re-score the closed prefix with
    /// `core::score` (cheap; few notes) and recompute the live combo/score so the
    /// figures match the final report exactly.
    fn score_due(&mut self) {
        let now = self.now_us();
        let mut newly = false;
        for (i, span) in self.spans.iter().enumerate() {
            if self.scored.contains(&i) {
                continue;
            }
            if now >= span.start_us + self.cfg.good_us {
                self.scored.insert(i);
                newly = true;
            }
        }
        if newly {
            self.recompute_live();
        }
    }

    /// Recompute the live combo/score over the scored prefix (spans whose window
    /// has closed), in time order, using the authoritative `core::score`.
    fn recompute_live(&mut self) {
        let mut closed: Vec<ExpectedNote> = self
            .spans
            .iter()
            .enumerate()
            .filter(|(i, _)| self.scored.contains(i))
            .filter_map(|(_, s)| MidiNote::new(s.note).map(|n| ExpectedNote::new(n, s.start_us)))
            .collect();
        closed.sort_by_key(|e| e.time_us);
        let report = score(&closed, &self.played, self.cfg);

        self.score = 0;
        self.combo = 0;
        self.best_combo = 0;
        self.live_hits = 0;
        self.live_misses = 0;
        for j in &report.judgments {
            match j {
                NoteJudgment::Hit { timing, .. } => {
                    let perfect = matches!(timing, Timing::Perfect);
                    self.score += award(perfect, self.combo);
                    self.combo += 1;
                    self.best_combo = self.best_combo.max(self.combo);
                    self.live_hits += 1;
                }
                NoteJudgment::Miss => {
                    self.combo = 0;
                    self.live_misses += 1;
                }
            }
        }
    }

    /// Set wait-mode (`w` key). Turning it off un-freezes immediately.
    pub fn set_wait_mode(&mut self, on: bool) {
        self.wait.set_armed(on);
        if !on && !self.clock.is_running() {
            self.clock.resume();
        }
    }

    /// Toggle wait-mode, returning the new armed state. Test-only; the live path
    /// flips it via `play_set_wait(on)` from the UI's current state.
    #[cfg(test)]
    pub fn toggle_wait_mode(&mut self) -> bool {
        let next = !self.wait.is_armed();
        self.set_wait_mode(next);
        next
    }

    /// Toggle "hear the song", returning the new state. Turning it off clears the
    /// audition trigger bookkeeping (the caller silences the synth).
    pub fn toggle_hear_song(&mut self) -> bool {
        self.hear_song = !self.hear_song;
        if !self.hear_song {
            self.song_on_fired.clear();
            self.song_off_fired.clear();
        }
        self.hear_song
    }

    /// The file position the backing should be at for the current clock, or
    /// `None` if it should not be playing yet / there is no backing track.
    pub fn backing_target_us(&self) -> Option<u64> {
        let b = self.backing.as_ref()?;
        backing_position_us(self.now_us(), self.shift_us, b.audio_start_us)
    }

    /// Indices whose `note_on` / `note_off` should fire for the song audition at
    /// the current clock but haven't yet. Caller routes these to the synth and
    /// then calls [`mark_song_fired`](Self::mark_song_fired). Empty unless
    /// `hear_song` is on.
    pub fn pending_song_triggers(&self) -> (Vec<usize>, Vec<usize>) {
        if !self.hear_song {
            return (Vec::new(), Vec::new());
        }
        let now = self.now_us();
        let mut need_on = Vec::new();
        let mut need_off = Vec::new();
        for (i, span) in self.spans.iter().enumerate() {
            if now >= span.start_us && !self.song_on_fired.contains(&i) {
                need_on.push(i);
            }
            if now >= span.end_us && !self.song_off_fired.contains(&i) {
                need_off.push(i);
            }
        }
        (need_on, need_off)
    }

    /// The pitch for a span index (for the caller routing audition note_on/off).
    pub fn span_note(&self, i: usize) -> Option<u8> {
        self.spans.get(i).map(|s| s.note)
    }

    /// Record that audition triggers fired so they don't re-fire next tick.
    pub fn mark_song_fired(&mut self, on: &[usize], off: &[usize]) {
        for &i in on {
            self.song_on_fired.insert(i);
        }
        for &i in off {
            self.song_off_fired.insert(i);
        }
    }

    /// A live snapshot for the `play_state` event.
    pub fn live_state(&mut self) -> PlayStateEvent {
        // Re-poll the gate to surface the awaited notes (idempotent read).
        let frozen = self.wait.poll(self.now_us()) == GateState::Frozen;
        let awaiting = self
            .wait
            .awaiting()
            .map(|s| s.notes.clone())
            .unwrap_or_default();
        PlayStateEvent {
            time_us: self.now_us(),
            frozen,
            score: self.score,
            combo: self.combo,
            best_combo: self.best_combo,
            hits: self.live_hits,
            misses: self.live_misses,
            held: self.held.iter().copied().collect(),
            awaiting,
            finished: self.is_finished(),
        }
    }

    /// The authoritative end-of-take report from `core::score` over every span
    /// and every collected strike. This is what the summary panel shows; the
    /// test asserts the derived `PlaySummary` equals `Summary::from_report`.
    pub fn report(&self) -> ScoreReport {
        let expected = expected_notes(&self.spans);
        score(&expected, &self.played, self.cfg)
    }

    /// Finalise the take into a serializable [`PlaySummary`].
    pub fn finish(&self) -> PlaySummary {
        let report = self.report();
        let summary = Summary::from_report(&report);
        let accuracy_bp = (summary.accuracy() * 10_000.0).round() as u32;
        PlaySummary {
            total_expected: summary.total_expected,
            hits: summary.hits,
            misses: summary.misses,
            extras: summary.extras,
            perfect: summary.perfect,
            early: summary.early,
            late: summary.late,
            accuracy_bp,
            best_combo: self.best_combo,
            score: self.score,
        }
    }
}

// ── Managed state + bundle loading ───────────────────────────────────────────

/// Tauri-managed play session: `None` until a bundle is loaded, cleared on
/// `play_finish`. The tick thread drives whatever is here while it is `Some`.
#[derive(Default)]
pub struct PlayState(pub Mutex<Option<PlaySession>>);

/// Read a bundle directory's `song.mid` + optional `meta.json` into a session.
///
/// Mirrors the TUI `load_play_screen`: parse `song.mid`, then attach the backing
/// track relative to the bundle dir when `meta.json` declares one. The backing
/// path stays absolute (resolved against the dir) so the bundle is movable.
fn load_session_from_dir(dir: &Path) -> Result<PlaySession, String> {
    let midi_path = dir.join("song.mid");
    let bytes = std::fs::read(&midi_path).map_err(|e| format!("read song.mid failed: {e}"))?;
    let title = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "song".into());
    let mut session = PlaySession::from_smf_bytes(title, &bytes)?;

    if let Ok(json) = std::fs::read_to_string(dir.join("meta.json")) {
        if let Ok(meta) = RecordingMeta::from_json(&json) {
            if let Some(backing) = meta.backing {
                session = session.with_backing(dir.join(&backing.file), backing.audio_start_us);
            }
        }
    }
    Ok(session)
}

// ── Tauri commands ───────────────────────────────────────────────────────────

/// Load a bundle directory into a fresh play session and return its static info.
///
/// Any previous session is replaced (and its backing stopped). Mirrors the TUI's
/// bundle load path.
#[tauri::command]
pub fn play_load(
    state: tauri::State<'_, PlayState>,
    audio: tauri::State<'_, AudioState>,
    dir: String,
) -> Result<PlayInfo, String> {
    let session = load_session_from_dir(Path::new(&dir))?;
    let info = session.info();
    // Stop any backing from a previous take before swapping sessions.
    audio.stop_backing();
    if let Some(synth) = &audio.synth {
        synth.all_off();
    }
    *state.0.lock().expect("play state mutex poisoned") = Some(session);
    Ok(info)
}

/// Arm / disarm wait mode (`w` key). Returns the new armed state.
#[tauri::command]
pub fn play_set_wait(state: tauri::State<'_, PlayState>, on: bool) -> bool {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    if let Some(s) = guard.as_mut() {
        s.set_wait_mode(on);
        s.is_wait_mode()
    } else {
        false
    }
}

/// Toggle "hear the song" (`m` key). Returns the new state; silences the synth
/// when turning off.
#[tauri::command]
pub fn play_toggle_hear_song(
    state: tauri::State<'_, PlayState>,
    audio: tauri::State<'_, AudioState>,
) -> bool {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    if let Some(s) = guard.as_mut() {
        let on = s.toggle_hear_song();
        if !on {
            if let Some(synth) = &audio.synth {
                synth.all_off();
            }
        }
        on
    } else {
        false
    }
}

/// Finish the take: tear the session down (stop backing, silence the synth) and
/// return the end-of-take summary. Idempotent — returns an empty summary when no
/// session is active.
#[tauri::command]
pub fn play_finish(
    state: tauri::State<'_, PlayState>,
    audio: tauri::State<'_, AudioState>,
) -> PlaySummary {
    let summary = {
        let mut guard = state.0.lock().expect("play state mutex poisoned");
        let summary = guard.as_ref().map(|s| s.finish()).unwrap_or_default();
        *guard = None;
        summary
    };
    audio.stop_backing();
    if let Some(synth) = &audio.synth {
        synth.all_off();
    }
    summary
}

/// Drive the active play session one tick: ingest drained MIDI events, advance
/// the injected clock by `dt_us`, route "hear the song" auditions and the
/// backing track to audio, and return a fresh [`PlayStateEvent`] for the webview.
///
/// Returns `None` when no session is active (the caller then runs the composer
/// path instead). Mirrors the TUI `tick` + `tick_song_synth` + `tick_backing`.
pub fn tick_play(
    state: &PlayState,
    audio: &AudioState,
    midi_events: &[NoteEvent],
    dt_us: u64,
) -> Option<PlayStateEvent> {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    let session = guard.as_mut()?;

    for &ev in midi_events {
        session.ingest(ev);
    }
    let frozen = session.advance(dt_us);

    // Route "hear the song" auditions through the synth.
    let (need_on, need_off) = session.pending_song_triggers();
    if let Some(synth) = &audio.synth {
        let vel = rockcraft_core::Velocity::new(HEAR_VELOCITY);
        for &i in &need_on {
            if let (Some(p), Some(v)) = (session.span_note(i).and_then(MidiNote::new), vel) {
                synth.note_on(p, v);
            }
        }
        for &i in &need_off {
            if let Some(p) = session.span_note(i).and_then(MidiNote::new) {
                synth.note_off(p);
            }
        }
    }
    session.mark_song_fired(&need_on, &need_off);

    // Sync the backing track to the clock (start at the shift boundary, pause
    // while frozen) using core's shared position formula so audio never drifts.
    let target = session.backing_target_us();
    audio.sync_play_backing(session.backing(), target, frozen);

    Some(session.live_state())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::Velocity;

    const SHIFT: u64 = PRE_ROLL_US + LEAD_US;

    fn on(note: u8, t: u64) -> NoteEvent {
        NoteEvent::on(MidiNote::new(note).unwrap(), Velocity::new(80).unwrap(), t)
    }
    fn off(note: u8, t: u64) -> NoteEvent {
        NoteEvent::off(MidiNote::new(note).unwrap(), t)
    }

    /// A 4-note fixture: C, D, E, F at song time 0, 250ms, 500ms, 750ms. After
    /// the whole-song shift the first lands at SHIFT.
    fn four_note_events() -> Vec<NoteEvent> {
        vec![
            on(60, 0),
            off(60, 200_000),
            on(62, 250_000),
            off(62, 450_000),
            on(64, 500_000),
            off(64, 700_000),
            on(65, 750_000),
            off(65, 950_000),
        ]
    }

    fn session() -> PlaySession {
        PlaySession::from_events("test".into(), &four_note_events())
    }

    /// Targets, after the pre-roll shift, fall at SHIFT + k·250ms.
    fn target_us(k: u64) -> u64 {
        SHIFT + k * 250_000
    }

    const PITCHES: [u8; 4] = [60, 62, 64, 65];

    #[test]
    fn loads_four_spans_shifted_by_pre_roll() {
        let s = session();
        let info = s.info();
        assert_eq!(info.notes.len(), 4);
        assert_eq!(
            s.shift_us(),
            SHIFT,
            "first note at t=0 → full pre-roll shift"
        );
        assert!((info.notes[0].start - SHIFT as f64 / 1000.0).abs() < 1e-6);
    }

    /// A scripted perfect take — every note struck dead-on — scores 4 Perfect,
    /// accuracy 100%, and the live figures match the finished report.
    #[test]
    fn perfect_take_four_perfect_accuracy_100() {
        let mut s = session();
        for k in 0..4u64 {
            let pitch = PITCHES[k as usize];
            s.ingest(on(pitch, target_us(k)));
            s.ingest(off(pitch, target_us(k) + 100_000));
        }
        s.advance(target_us(3) + 200_000);
        let summary = s.finish();
        assert_eq!(summary.hits, 4);
        assert_eq!(summary.misses, 0);
        assert_eq!(summary.perfect, 4);
        assert_eq!(summary.accuracy_bp, 10_000, "100% accuracy");
        let live = s.live_state();
        assert_eq!(live.hits, 4);
        assert_eq!(live.misses, 0);
    }

    /// An offset take (+120 ms — inside the 150 ms good window, outside the 50 ms
    /// perfect window) yields Good (Late) judgements, not Perfect.
    #[test]
    fn offset_take_yields_good_not_perfect() {
        let mut s = session();
        for k in 0..4u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k) + 120_000));
        }
        s.advance(target_us(3) + 200_000);
        let summary = s.finish();
        assert_eq!(summary.hits, 4, "all within the 150ms good window");
        assert_eq!(summary.perfect, 0, "none within the 50ms perfect window");
        assert_eq!(summary.late, 4);
        assert_eq!(summary.accuracy_bp, 10_000);
    }

    /// A missed note (no strike) is a Miss; accuracy falls below 100%.
    #[test]
    fn missing_note_is_a_miss() {
        let mut s = session();
        for k in 0..3u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k)));
        }
        s.advance(target_us(3) + 200_000);
        let summary = s.finish();
        assert_eq!(summary.hits, 3);
        assert_eq!(summary.misses, 1);
        assert_eq!(summary.accuracy_bp, 7500, "3/4 = 75%");
    }

    /// Summary totals equal `core::stats::Summary` for the scripted take.
    #[test]
    fn summary_matches_core_stats() {
        let mut s = session();
        for k in 0..4u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k)));
        }
        s.advance(target_us(3) + 200_000);
        let report = s.report();
        let core_summary = Summary::from_report(&report);
        let mine = s.finish();
        assert_eq!(mine.total_expected, core_summary.total_expected);
        assert_eq!(mine.hits, core_summary.hits);
        assert_eq!(mine.misses, core_summary.misses);
        assert_eq!(mine.extras, core_summary.extras);
        assert_eq!(mine.perfect, core_summary.perfect);
        assert_eq!(mine.early, core_summary.early);
        assert_eq!(mine.late, core_summary.late);
        let core_bp = (core_summary.accuracy() * 10_000.0).round() as u32;
        assert_eq!(mine.accuracy_bp, core_bp);
    }

    /// Wait mode: the clock does not advance past a step until its notes are
    /// held, exactly like `play.rs`.
    #[test]
    fn wait_mode_freezes_until_held() {
        let mut s = session();
        s.set_wait_mode(true);
        s.advance(SHIFT);
        assert_eq!(s.now_us(), SHIFT);
        s.advance(5_000_000);
        s.advance(5_000_000);
        assert_eq!(s.now_us(), SHIFT, "frozen on the unsatisfied first step");
        s.ingest(on(60, SHIFT));
        s.advance(250_000);
        assert_eq!(s.now_us(), SHIFT + 250_000, "advances once held");
    }

    /// Toggling wait off unfreezes a frozen clock immediately.
    #[test]
    fn toggle_wait_off_resumes() {
        let mut s = session();
        assert!(s.toggle_wait_mode());
        s.advance(SHIFT);
        s.advance(1_000_000);
        assert_eq!(s.now_us(), SHIFT);
        assert!(!s.toggle_wait_mode());
        s.advance(1_000_000);
        assert_eq!(s.now_us(), SHIFT + 1_000_000);
    }

    /// "Hear the song" audition fires note_on at the shifted span start, once.
    #[test]
    fn hear_song_triggers_at_shifted_start() {
        let mut s = session().with_hear_song(true);
        s.advance(SHIFT);
        let (on_idx, _off) = s.pending_song_triggers();
        assert_eq!(on_idx, vec![0], "first span due to sound at SHIFT");
        s.mark_song_fired(&on_idx, &[]);
        let (on_idx2, _) = s.pending_song_triggers();
        assert!(on_idx2.is_empty());
    }

    /// Backing target is silent during the lead-in and tracks the clock after.
    #[test]
    fn backing_silent_then_tracks() {
        let mut s = session().with_backing(PathBuf::from("backing.mp3"), 0);
        assert_eq!(s.backing_target_us(), None, "before the lead-in: silent");
        s.advance(SHIFT);
        assert_eq!(s.backing_target_us(), Some(0));
        s.advance(1_000_000);
        assert_eq!(s.backing_target_us(), Some(1_000_000));
    }

    /// Load the committed fixture bundle from disk and play a scripted perfect
    /// take over it: 4 Perfect, accuracy 100%. This exercises the real
    /// `song.mid` parse path against `fixtures/play-bundle/`.
    #[test]
    fn fixture_bundle_perfect_take() {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("play-bundle");
        let mut s = load_session_from_dir(&dir).expect("load fixture bundle");
        let info = s.info();
        assert_eq!(info.notes.len(), 4, "fixture has four notes");
        assert_eq!(
            s.shift_us(),
            SHIFT,
            "first note at t=0 → full pre-roll shift"
        );

        for k in 0..4u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k)));
        }
        s.advance(target_us(3) + 200_000);
        let summary = s.finish();
        assert_eq!(summary.hits, 4);
        assert_eq!(summary.misses, 0);
        assert_eq!(summary.perfect, 4);
        assert_eq!(summary.accuracy_bp, 10_000);
    }

    /// is_finished trips only after the song plus its tail pause.
    #[test]
    fn finishes_after_tail() {
        let mut s = session();
        let dur = s.info().duration_us;
        s.advance(dur);
        assert!(!s.is_finished(), "still within the tail pause");
        s.advance(LEAD_US + 1);
        assert!(s.is_finished());
    }
}
