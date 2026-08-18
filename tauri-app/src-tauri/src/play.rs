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
    backing_position_us, hand::hand_of_pitch_value, score, song_shift_us, BackgroundImage,
    ExpectedNote, Feedback, GateState, Hand, HandOverride, MidiNote, NoteEvent, NoteEventKind,
    NoteJudgment, PlayClock, RecordingMeta, ScoreConfig, ScoreReport, Summary, SynthBus, Timing,
    Transform, WaitGate, DEFAULT_SPLIT,
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

/// Ceiling on undelivered per-note judgments (M14-B). At ~60 Hz a tick closes a
/// handful of notes at most, so this only ever trips when nothing is draining
/// them; the oldest are dropped because a one-shot effect that late is useless.
const MAX_PENDING_FEEDBACK: usize = 64;

/// A sustained note: pitch held from `start_us` to `end_us` (microseconds,
/// already shifted by the pre-roll). Mirrors `tui::highway::NoteSpan`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteSpan {
    pub note: u8,
    pub start_us: u64,
    pub end_us: u64,
    /// The piece's per-note hand exception (M14-E), read from `meta.json` at
    /// load. `None` — the common case — means the note follows the split line.
    pub hand: Option<Hand>,
}

impl NoteSpan {
    /// Which hand plays this note: the authored override when set, else the
    /// split rule. Every hand-aware consumer here reads this, so a crossover is
    /// practised and scored on the hand the author marked.
    pub fn effective_hand(&self, split: u8) -> Hand {
        self.hand
            .unwrap_or_else(|| hand_of_pitch_value(self.note, split))
    }
}

/// A background video attached to a bundle, surfaced to the play screen so the
/// webview can render it behind the highway (M9-G). `path` is absolute (resolved
/// against the bundle dir); `offset_us` is the signed alignment offset applied as
/// `videoTime = songTime + offset_us`. `core` carries only the reference — the
/// HTML5 `<video>` element decodes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackgroundVideoSession {
    pub path: PathBuf,
    pub offset_us: i64,
}

/// Serializable background-video reference for [`PlayInfo`]. `path` is the
/// absolute file path (the webview wraps it with `convertFileSrc`).
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundVideoView {
    pub path: String,
    pub offset_us: i64,
}

/// One background image layer in a play session: the resolved absolute file plus
/// the layer's keyframed animation (M14-D).
#[derive(Debug, Clone)]
pub struct BackgroundLayerSession {
    pub path: PathBuf,
    pub layer: BackgroundImage,
}

/// Serializable background-image reference for [`PlayInfo`] — the static half.
/// `path` is the absolute file path (the webview wraps it with
/// `convertFileSrc`); the *moving* half arrives per tick in [`PlayStateEvent`].
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundLayerView {
    pub id: String,
    pub path: String,
}

/// One background layer's transform at the current song time (M14-D). Evaluated
/// by `core` each tick, applied verbatim by the webview.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct BackgroundTransformView {
    pub id: String,
    pub transform: Transform,
}

/// A backing audio track attached to a bundle, plus the file position that lines
/// up with song time 0 (`audio_start_us`).
#[derive(Debug, Clone)]
pub struct Backing {
    pub path: PathBuf,
    pub audio_start_us: i64,
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
    /// Background video to render behind the highway, or `None` when the piece
    /// has no backdrop (M9-G).
    pub video: Option<BackgroundVideoView>,
    /// Background image layers to render behind the highway, back-to-front, or
    /// empty when the piece has none (M14-D).
    pub backgrounds: Vec<BackgroundLayerView>,
    /// Whether "hear the song" starts on.
    pub hear_song: bool,
    /// Piece tempo (BPM) so the highway draws its bar/beat grid at the right
    /// spacing; defaults to 120 when the bundle has no grid.
    pub bpm: u32,
    /// Beats per bar (time-signature numerator); defaults to 4.
    pub beats_per_bar: u8,
}

/// One span projected for the webview: pitch + ms bounds + the hand that plays
/// it. `hand` is the **effective** hand (the piece's per-note override, else its
/// split line), so the highway colours in "hands" mode without re-deriving the
/// rule — and a crossover note reads on the hand the author marked (M14-E).
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SpanView {
    pub note: u8,
    /// Start in milliseconds.
    pub start: f64,
    /// End in milliseconds.
    pub end: f64,
    /// `"left"` | `"right"` — `Hand`'s wire name.
    pub hand: Hand,
}

/// One judged note, surfaced once so the webview can fire a decaying one-shot
/// effect at that note's lane (M14-B). The `level` is
/// [`rockcraft_core::Feedback`]'s wire name — the frontend never re-derives how
/// loud a judgment should read; `timing` carries the finer detail (early vs
/// late) for the readout, and `error_us` the signed timing error.
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
pub struct HitFeedbackView {
    /// Pitch, i.e. which lane the effect belongs at.
    pub note: u8,
    /// `"clear"` | `"near"` | `"subtle"` — `Feedback::as_str`.
    pub level: &'static str,
    /// `"perfect"` | `"early"` | `"late"` | `"miss"`.
    pub timing: &'static str,
    /// Signed timing error in microseconds (negative = early). 0 on a miss.
    pub error_us: i64,
    /// The (shifted) song time of the note this judges.
    pub time_us: u64,
}

impl HitFeedbackView {
    /// Project one span's judgment onto the wire.
    fn new(span: &NoteSpan, judgment: NoteJudgment) -> Self {
        let (timing, error_us) = match judgment {
            NoteJudgment::Hit { timing, error_us } => {
                let name = match timing {
                    Timing::Perfect => "perfect",
                    Timing::Early => "early",
                    Timing::Late => "late",
                };
                (name, error_us)
            }
            NoteJudgment::Miss => ("miss", 0),
        };
        Self {
            note: span.note,
            level: Feedback::from_judgment(judgment).as_str(),
            timing,
            error_us,
            time_us: span.start_us,
        }
    }
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
    /// Notes judged since the previous snapshot, in song-time order (M14-B).
    /// One-shot: each judged note appears in exactly one `play_state`, so the
    /// webview can spawn a decaying effect per entry without de-duplicating.
    pub judgments: Vec<HitFeedbackView>,
    /// Each background layer's transform at this instant, back-to-front and in
    /// the same order as `PlayInfo::backgrounds` (M14-D). Empty when the piece
    /// has no background images.
    pub backgrounds: Vec<BackgroundTransformView>,
    /// Set once the song (plus tail) has finished.
    pub finished: bool,
}

/// A complete read-only picture of the live take, for the agent-control socket.
///
/// `play_state` events reach the webview only, so an agent driving the app over
/// the socket previously had **no way to see a running take** — not the clock,
/// not what the wait gate wanted, not even whether a session existed. Diagnosing
/// "wait mode won't advance" then came down to guesswork. This carries the live
/// snapshot *and* the session's configuration (the half the event never had), so
/// the state is directly readable.
///
/// Deliberately non-mutating: unlike [`PlaySession::live_state`] it neither
/// polls the gate nor drains `pending_feedback`, so an agent reading status can
/// never steal a judgment effect from the webview or perturb the take it is
/// observing.
#[derive(Debug, Clone, Serialize)]
pub struct PlayStatusView {
    /// False when no bundle is loaded — every other field is then meaningless.
    pub loaded: bool,
    pub title: String,
    pub time_us: u64,
    pub duration_us: u64,
    /// Manual pause (`play_toggle_pause`).
    pub paused: bool,
    /// Clock held — by the manual pause or an unsatisfied wait step.
    pub frozen: bool,
    pub finished: bool,
    /// Wait mode armed.
    pub wait_armed: bool,
    /// Pitches the gate is waiting for; empty unless frozen by the gate. The
    /// pairing of this with `held` is what makes a stuck take diagnosable.
    pub awaiting: Vec<u8>,
    /// Pitches currently held by the player.
    pub held: Vec<u8>,
    /// `"both"`, `"left"` or `"right"`.
    pub practice: String,
    pub split_pitch: u8,
    pub rate_permille: u16,
    pub hear_song: bool,
    pub monitor: bool,
    pub score: u64,
    pub combo: u32,
    pub best_combo: u32,
    pub hits: usize,
    pub misses: usize,
    pub bpm: u32,
    pub beats_per_bar: u8,
    /// Total notes in the loaded chart, to sanity-check against the bundle.
    pub note_count: usize,
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

/// Practice speed meaning "normal", in permille.
pub const PLAY_RATE_UNITY: u16 = 1000;
/// Slowest practice speed (0.25x) — matches `core::Action::SetPlaybackRate`'s
/// range so the two transports feel the same.
pub const PLAY_RATE_MIN: u16 = 250;
/// Fastest practice speed (2x).
pub const PLAY_RATE_MAX: u16 = 2000;

/// Grid metadata from `meta.json`, used to bar-align the play shift and to draw
/// the highway bar/beat grid at the piece's real tempo.
#[derive(Debug, Clone, Copy)]
struct GridInfo {
    bpm: u32,
    beats_per_bar: u8,
    bar_us: u64,
    origin_us: u64,
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
                        hand: None,
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
                        hand: None,
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
            hand: None,
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

/// Keep only spans belonging to the practiced hand (`None` = both hands).
fn spans_for(
    spans: &[NoteSpan],
    practice: Option<Hand>,
    split: u8,
) -> impl Iterator<Item = &NoteSpan> {
    spans
        .iter()
        .filter(move |s| practice.is_none_or(|h| s.effective_hand(split) == h))
}

/// Wait-gate steps for the practiced hand only (or all hands when `None`).
fn expected_steps_for(
    spans: &[NoteSpan],
    practice: Option<Hand>,
    split: u8,
) -> Vec<(MidiNote, u64)> {
    spans_for(spans, practice, split)
        .filter_map(|s| MidiNote::new(s.note).map(|n| (n, s.start_us)))
        .collect()
}

/// Scoring targets for the practiced hand only (or all hands when `None`).
fn expected_notes_for(spans: &[NoteSpan], practice: Option<Hand>, split: u8) -> Vec<ExpectedNote> {
    spans_for(spans, practice, split)
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
    /// Background video reference resolved from the bundle's `meta.json` (M9-G).
    video: Option<BackgroundVideoSession>,
    /// Background image layers resolved from the bundle's `meta.json` (M14-D).
    backgrounds: Vec<BackgroundLayerSession>,
    hear_song: bool,
    /// Input monitor: synthesise the player's own key presses so they hear
    /// themselves through the app synth. Independent of "hear the song" (which
    /// auditions the chart). Toggled at runtime; off by default.
    monitor: bool,
    /// Hand-practice mode: `None` = both hands; `Some(h)` = only hand `h` is
    /// waited-on/scored while the other hand auto-plays. A note's hand is its
    /// authored override (M14-E) when it has one, else its pitch relative to
    /// `split_pitch` — see [`NoteSpan::effective_hand`].
    practice: Option<Hand>,
    /// Pitch dividing left/right hands for [`practice`](Self::practice), from
    /// the piece's `meta.json` (or [`DEFAULT_SPLIT`]). Per-note overrides win.
    split_pitch: u8,
    /// Practice speed in permille (1000 = 1x). Scales the time injected into
    /// [`advance`](Self::advance), so the clock, wait gate and scoring windows
    /// all stretch together and the chart itself is never rewritten.
    rate_permille: u16,
    /// Manual pause (`HostCommand::PlayTogglePause`). Freezes the clock + backing
    /// independently of wait-mode; while set the highway and scoring clock hold
    /// their position. The play-screen UI/keys are M12-B (#232).
    paused: bool,
    cfg: ScoreConfig,
    /// Piece tempo + beats/bar (from `meta.grid`, else 120/4) so the play highway
    /// draws its bar/beat grid at the real tempo. Bar-alignment of the pre-roll
    /// shift keeps timeline bars on play bar lines.
    bpm: u32,
    beats_per_bar: u8,

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
    /// Judgments queued since the last [`live_state`](PlaySession::live_state),
    /// drained by it so each judged note fires exactly one effect (M14-B).
    pending_feedback: Vec<HitFeedbackView>,

    /// "Hear the song" audition trigger bookkeeping (span indices fired).
    song_on_fired: HashSet<usize>,
    song_off_fired: HashSet<usize>,
}

impl PlaySession {
    /// Build a session directly from a note-event timeline (no grid: 120/4, raw
    /// shift). Test-only seam; production loads go through
    /// [`from_events_with_grid`](Self::from_events_with_grid) with the bundle grid.
    #[cfg(test)]
    pub fn from_events(title: String, events: &[NoteEvent]) -> Self {
        Self::from_events_with_grid(title, events, None)
    }

    /// Like [`from_events`] but, when the bundle's grid is known, rounds the
    /// pre-roll shift up to a whole bar so timeline bar lines map onto play-clock
    /// bar lines (notes stay on the beat/bar grid, matching the editor), and
    /// records the tempo/meter for the highway grid.
    fn from_events_with_grid(title: String, events: &[NoteEvent], grid: Option<GridInfo>) -> Self {
        let raw = build_spans(events);
        let first_us = raw.iter().map(|s| s.start_us).min().unwrap_or(0);
        let mut shift_us = song_shift_us(first_us, PRE_ROLL_US, LEAD_US);
        if let Some(g) = grid {
            if g.bar_us > 0 {
                let rem = (g.origin_us + shift_us) % g.bar_us;
                if rem != 0 {
                    shift_us += g.bar_us - rem;
                }
            }
        }
        let spans: Vec<NoteSpan> = raw
            .into_iter()
            .map(|s| NoteSpan {
                start_us: s.start_us + shift_us,
                end_us: s.end_us + shift_us,
                ..s
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
            rate_permille: PLAY_RATE_UNITY,
            shift_us,
            backing: None,
            video: None,
            backgrounds: Vec::new(),
            hear_song: false,
            monitor: false,
            practice: None,
            split_pitch: DEFAULT_SPLIT,
            paused: false,
            cfg: ScoreConfig::default(),
            bpm: grid.map(|g| g.bpm).unwrap_or(120),
            beats_per_bar: grid.map(|g| g.beats_per_bar).unwrap_or(4),
            held: BTreeSet::new(),
            played: Vec::new(),
            scored: HashSet::new(),
            score: 0,
            combo: 0,
            best_combo: 0,
            live_hits: 0,
            live_misses: 0,
            pending_feedback: Vec::new(),
            song_on_fired: HashSet::new(),
            song_off_fired: HashSet::new(),
        }
    }

    /// Attach a backing track resolved from the bundle's `meta.json`.
    pub fn with_backing(mut self, path: PathBuf, audio_start_us: i64) -> Self {
        self.backing = Some(Backing {
            path,
            audio_start_us,
        });
        self
    }

    /// Apply the piece's authored hand assignment from its `meta.json` (M14-E):
    /// the split line plus the per-note exceptions.
    ///
    /// Overrides are keyed by the note's **original** `(pitch, start_us)` — the
    /// position the editor saved — so they are matched here against
    /// `start_us - shift_us`, before the whole-song pre-roll shift this session
    /// already applied.
    pub fn with_hands(mut self, overrides: &[HandOverride], split: u8) -> Self {
        self.split_pitch = split;
        let shift_us = self.shift_us;
        for span in &mut self.spans {
            let original_start = span.start_us.saturating_sub(shift_us);
            span.hand = overrides
                .iter()
                .find(|o| o.pitch == span.note && o.start_us == original_start)
                .map(|o| o.hand);
        }
        self
    }

    /// Attach a background video resolved from the bundle's `meta.json` (M9-G).
    pub fn with_video(mut self, path: PathBuf, offset_us: i64) -> Self {
        self.video = Some(BackgroundVideoSession { path, offset_us });
        self
    }

    /// The attached background video, if any.
    pub fn video(&self) -> Option<&BackgroundVideoSession> {
        self.video.as_ref()
    }

    /// Attach the background image layers resolved from the bundle's
    /// `meta.json`, back-to-front (M14-D). `dir` resolves each layer's
    /// bundle-relative file to the absolute path the webview loads.
    pub fn with_backgrounds(mut self, dir: &Path, layers: Vec<BackgroundImage>) -> Self {
        self.backgrounds = layers
            .into_iter()
            .map(|mut layer| {
                layer.normalize();
                BackgroundLayerSession {
                    path: dir.join(&layer.file),
                    layer,
                }
            })
            .collect();
        self
    }

    /// Song-content time for background keyframes: the clock minus the
    /// whole-song pre-roll shift, so a keyframe authored at bar 1 of the piece
    /// lands on bar 1 here rather than during the empty lead-in. Mirrors the
    /// backdrop video's `(songTime - shift) + offset` mapping.
    fn background_time_us(&self) -> u64 {
        self.now_us().saturating_sub(self.shift_us)
    }

    /// Each layer's transform at the current song time, back-to-front (M14-D).
    fn background_transforms(&self) -> Vec<BackgroundTransformView> {
        if self.backgrounds.is_empty() {
            return Vec::new();
        }
        let at_us = self.background_time_us();
        self.backgrounds
            .iter()
            .map(|b| BackgroundTransformView {
                id: b.layer.id.clone(),
                transform: b.layer.transform_at(at_us),
            })
            .collect()
    }

    /// Start with "hear the song" on or off. Bundle loading passes
    /// `meta.backing.is_none()` here (#247): a MIDI-only piece auditions itself
    /// so it isn't silent without a live piano, while a piece with a backing
    /// track leaves it off so the synth doesn't double the recording.
    /// `play_toggle_hear_song` still flips it at runtime either way.
    pub fn with_hear_song(mut self, on: bool) -> Self {
        self.hear_song = on;
        self
    }

    /// Begin the take manually paused so the highway holds at the start until the
    /// player hits Start (which calls `toggle_pause` to resume). This keeps the
    /// song from running before the window is focused, and makes Replay re-enter
    /// the same ready-to-start state. The same freeze machinery wait-mode uses
    /// holds the clock at 0 until resumed.
    pub fn start_paused(mut self) -> Self {
        self.paused = true;
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
                    hand: s.effective_hand(self.split_pitch),
                })
                .collect(),
            shift_us: self.shift_us,
            duration_us: self.duration_us,
            lead_us: LEAD_US,
            has_backing: self.backing.is_some(),
            video: self.video().map(|v| BackgroundVideoView {
                path: v.path.to_string_lossy().into_owned(),
                offset_us: v.offset_us,
            }),
            backgrounds: self
                .backgrounds
                .iter()
                .map(|b| BackgroundLayerView {
                    id: b.layer.id.clone(),
                    path: b.path.to_string_lossy().into_owned(),
                })
                .collect(),
            hear_song: self.hear_song,
            bpm: self.bpm,
            beats_per_bar: self.beats_per_bar,
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

    /// Forward a `NoteEvent`: update the held set and (on a real strike) collect
    /// it for scoring at its `timestamp_us`.
    ///
    /// The live path ([`tick_play`]) re-stamps incoming device events with the
    /// current play-clock time before calling this, so strikes land in the same
    /// frame as `expected` (see the note there). Tests call `ingest` directly with
    /// explicit timestamps.
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
        // Practice speed scales the time entering the session, not the chart:
        // the clock, wait gate and scoring windows all stretch by the same
        // factor, so judgements stay identical relative to the music.
        let dt_us = if self.rate_permille == PLAY_RATE_UNITY {
            dt_us
        } else {
            dt_us * self.rate_permille as u64 / PLAY_RATE_UNITY as u64
        };
        self.wait.set_held(self.held.clone());
        let wait_frozen = self.wait.poll(self.clock.now_us()) == GateState::Frozen;
        // A manual pause freezes the transport just like an unsatisfied wait step.
        let frozen = self.paused || wait_frozen;
        if frozen && self.clock.is_running() {
            self.clock.pause();
        } else if !frozen && !self.clock.is_running() {
            self.clock.resume();
        }
        // Clamp this tick so an armed wait never carries the clock *past* the next
        // note's onset. The tick thread feeds a wall-clock `dt` (~4 ms nominal),
        // but a stall — a WebView2 video decode, GC, lock contention — spikes it
        // to tens/hundreds of ms. Without the clamp the clock leaps past the
        // onset and the gate freezes wherever it landed, so the note you must
        // play sits in the past (at/after its end) — the "wait mode pauses after
        // the fact" bug. Landing exactly on the onset makes the next tick freeze
        // there, pinning the note at the hit line. Only while advancing (a freeze
        // already parks the clock) and armed (free play is untouched).
        let mut step_us = dt_us;
        if !frozen && self.wait.is_armed() {
            if let Some(next) = self.wait.next_step_time() {
                let now = self.clock.now_us();
                if next > now {
                    step_us = step_us.min(next - now);
                }
            }
        }
        self.clock.advance(step_us);
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
        let newly: Vec<usize> = self
            .spans
            .iter()
            .enumerate()
            .filter(|(i, s)| {
                !self.scored.contains(i)
                    && now >= s.start_us + self.cfg.good_us
                    // When practicing one hand, the other hand auto-plays — don't score it.
                    && !self
                        .practice
                        .is_some_and(|h| s.effective_hand(self.split_pitch) != h)
            })
            .map(|(i, _)| i)
            .collect();
        if newly.is_empty() {
            return;
        }
        self.scored.extend(newly.iter().copied());
        let judged = self.recompute_live();

        // Queue a one-shot effect for each note that just became final — in the
        // time order `recompute_live` judged them, so the webview sees a chord's
        // notes together and successive notes in the order they were played.
        let fresh: Vec<HitFeedbackView> = judged
            .into_iter()
            .filter(|(i, _)| newly.contains(i))
            .filter_map(|(i, j)| self.spans.get(i).map(|s| HitFeedbackView::new(s, j)))
            .collect();
        self.pending_feedback.extend(fresh);
        // A webview reads this every tick; if nothing does (headless, or a
        // stalled frontend), drop the oldest rather than grow without bound —
        // stale effects are worthless anyway.
        let overflow = self
            .pending_feedback
            .len()
            .saturating_sub(MAX_PENDING_FEEDBACK);
        if overflow > 0 {
            self.pending_feedback.drain(..overflow);
        }
    }

    /// Recompute the live combo/score over the scored prefix (spans whose window
    /// has closed), in time order, using the authoritative `core::score`.
    ///
    /// Returns `(span index, judgment)` in that same time order so the caller can
    /// attribute each judgment back to the note it belongs to (M14-B).
    fn recompute_live(&mut self) -> Vec<(usize, NoteJudgment)> {
        let mut closed: Vec<(usize, ExpectedNote)> = self
            .spans
            .iter()
            .enumerate()
            .filter(|(i, _)| self.scored.contains(i))
            .filter_map(|(i, s)| {
                MidiNote::new(s.note).map(|n| (i, ExpectedNote::new(n, s.start_us)))
            })
            .collect();
        closed.sort_by_key(|(_, e)| e.time_us);
        let expected: Vec<ExpectedNote> = closed.iter().map(|(_, e)| *e).collect();
        let report = score(&expected, &self.played, self.cfg);

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

        closed
            .into_iter()
            .map(|(i, _)| i)
            .zip(report.judgments)
            .collect()
    }

    /// Toggle a manual pause of the play session (`HostCommand::PlayTogglePause`),
    /// returning the new paused state. Freezes the clock when pausing and thaws
    /// it when resuming, reusing the same freeze machinery wait-mode uses — so
    /// the highway and scoring clock hold their position and continue from it.
    /// The backing audio follows via [`advance`]'s returned frozen flag.
    pub fn toggle_pause(&mut self) -> bool {
        self.paused = !self.paused;
        if self.paused {
            if self.clock.is_running() {
                self.clock.pause();
            }
        } else {
            // Resume unless wait-mode is holding an unsatisfied step; the next
            // `advance` re-freezes in that case.
            self.wait.set_held(self.held.clone());
            let wait_frozen = self.wait.poll(self.clock.now_us()) == GateState::Frozen;
            if !wait_frozen && !self.clock.is_running() {
                self.clock.resume();
            }
        }
        self.paused
    }

    /// Is the session manually paused? Test-only for now; the live paused state
    /// reaches the webview through [`live_state`](Self::live_state)'s `frozen`
    /// flag, and `toggle_pause` returns the new state. The play-screen UI that
    /// reads a dedicated indicator is M12-B (#232).
    #[cfg(test)]
    pub fn is_paused(&self) -> bool {
        self.paused
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

    /// Is input-monitor on (synthesise the player's own key presses)?
    pub fn is_monitor(&self) -> bool {
        self.monitor
    }

    /// Toggle input-monitor, returning the new state. When turning it off the
    /// caller silences the synth (any monitored notes still ringing).
    pub fn toggle_monitor(&mut self) -> bool {
        self.monitor = !self.monitor;
        self.monitor
    }

    /// Set the practiced hand (`None` = both). Rebuilds the wait gate so only that
    /// hand's notes are waited on; the other hand auto-plays via `tick_play`.
    pub fn set_practice(&mut self, practice: Option<Hand>) {
        self.practice = practice;
        self.rebuild_wait_gate();
    }

    /// Set the practice speed in permille, clamped to
    /// [`PLAY_RATE_MIN`]..=[`PLAY_RATE_MAX`]. Returns the applied value.
    pub fn set_rate(&mut self, rate_permille: u16) -> u16 {
        self.rate_permille = rate_permille.clamp(PLAY_RATE_MIN, PLAY_RATE_MAX);
        self.rate_permille
    }

    /// Set the pitch dividing left/right hands. Re-classifies every note that
    /// follows the split line — notes carrying an authored override keep their
    /// hand — and rebuilds the wait gate.
    pub fn set_split(&mut self, split: u8) {
        self.split_pitch = split;
        self.rebuild_wait_gate();
    }

    /// Rebuild the wait gate from the practiced hand's steps, preserving armed
    /// state. A fresh gate starts at step 0 (the song's start), so when practice
    /// or the split changes **mid-take** we must seek it to the current playhead —
    /// otherwise the gate freezes on a step already in the past (a note played
    /// long ago) and never advances, ignoring the notes the player is actually
    /// meant to play now.
    fn rebuild_wait_gate(&mut self) {
        let armed = self.wait.is_armed();
        let steps = expected_steps_for(&self.spans, self.practice, self.split_pitch);
        self.wait = WaitGate::from_expected(&steps);
        self.wait.set_armed(armed);
        self.wait.seek_to(self.clock.now_us());
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
        let now = self.now_us();
        let mut need_on = Vec::new();
        let mut need_off = Vec::new();
        for (i, span) in self.spans.iter().enumerate() {
            // A span auto-sounds if "hear the song" is on (all notes), OR we are
            // practicing one hand and this span is the OTHER hand (accompaniment).
            let autoplay = self.hear_song
                || self
                    .practice
                    .is_some_and(|h| span.effective_hand(self.split_pitch) != h);
            if !autoplay {
                continue;
            }
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
    /// A read-only status picture of this take (see [`PlayStatusView`]).
    ///
    /// Reads the gate's *last* poll rather than re-polling: the tick loop polls
    /// at ~60 Hz, so this is current, and observing stays free of side effects.
    pub fn status(&self) -> PlayStatusView {
        let awaiting = self
            .wait
            .awaiting()
            .map(|s| s.notes.clone())
            .unwrap_or_default();
        PlayStatusView {
            loaded: true,
            title: self.title.clone(),
            time_us: self.now_us(),
            duration_us: self.duration_us,
            paused: self.paused,
            frozen: self.paused || !awaiting.is_empty(),
            finished: self.is_finished(),
            wait_armed: self.wait.is_armed(),
            awaiting,
            held: self.held.iter().copied().collect(),
            practice: match self.practice {
                None => "both".to_string(),
                Some(Hand::Left) => "left".to_string(),
                Some(Hand::Right) => "right".to_string(),
            },
            split_pitch: self.split_pitch,
            rate_permille: self.rate_permille,
            hear_song: self.hear_song,
            monitor: self.monitor,
            score: self.score,
            combo: self.combo,
            best_combo: self.best_combo,
            hits: self.live_hits,
            misses: self.live_misses,
            bpm: self.bpm,
            beats_per_bar: self.beats_per_bar,
            note_count: self.spans.len(),
        }
    }

    pub fn live_state(&mut self) -> PlayStateEvent {
        // Re-poll the gate to surface the awaited notes (idempotent read). A
        // manual pause also reads as frozen so the webview sees the clock held.
        let frozen = self.paused || self.wait.poll(self.now_us()) == GateState::Frozen;
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
            // Drained: each judged note reaches the webview exactly once, so the
            // effect fires on the tick the judgment became final and never again.
            judgments: std::mem::take(&mut self.pending_feedback),
            backgrounds: self.background_transforms(),
            finished: self.is_finished(),
        }
    }

    /// The authoritative end-of-take report from `core::score` over every span
    /// and every collected strike. This is what the summary panel shows; the
    /// test asserts the derived `PlaySummary` equals `Summary::from_report`.
    pub fn report(&self) -> ScoreReport {
        // Score only the practiced hand (the other hand auto-played).
        let expected = expected_notes_for(&self.spans, self.practice, self.split_pitch);
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
///
/// "Hear the song" defaults to on exactly when the piece has no backing track
/// (#247), so a MIDI-only bundle isn't silent without a live piano.
fn load_session_from_dir(dir: &Path) -> Result<PlaySession, String> {
    let midi_path = dir.join("song.mid");
    let bytes = std::fs::read(&midi_path).map_err(|e| format!("read song.mid failed: {e}"))?;
    let title = dir
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "song".into());
    let events = smf_bytes_to_events(&bytes).map_err(|e| e.to_string())?;
    // Read meta up front so the grid can bar-align the shift and set the tempo.
    let meta = std::fs::read_to_string(dir.join("meta.json"))
        .ok()
        .and_then(|j| RecordingMeta::from_json(&j).ok());
    let grid = meta.as_ref().and_then(|m| m.grid).map(|g| GridInfo {
        bpm: g.bpm,
        beats_per_bar: g.time_sig.beats_per_bar,
        bar_us: g.bar_us(),
        origin_us: g.origin_us,
    });
    let mut session = PlaySession::from_events_with_grid(title, &events, grid);

    let mut has_backing = false;
    if let Some(meta) = meta {
        // Hand assignment first: it sets `split_pitch`, and every later
        // hand-aware read (practice gate, scoring, colouring) goes through it.
        session = session.with_hands(&meta.hand_overrides, meta.split_or_default());
        if let Some(backing) = meta.backing {
            session = session.with_backing(dir.join(&backing.file), backing.audio_start_us);
            has_backing = true;
        }
        if let Some(video) = meta.video {
            session = session.with_video(dir.join(&video.file), video.offset_us);
        }
        if !meta.backgrounds.is_empty() {
            session = session.with_backgrounds(dir, meta.backgrounds);
        }
    }
    Ok(session.with_hear_song(!has_backing).start_paused())
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

/// The live take's full state, or `loaded: false` when none is running.
/// See [`PlayStatusView`] — this is the socket's window onto a running game.
#[tauri::command]
pub fn play_status(state: tauri::State<'_, PlayState>) -> PlayStatusView {
    let guard = state.0.lock().expect("play state mutex poisoned");
    guard
        .as_ref()
        .map(|s| s.status())
        .unwrap_or(PlayStatusView {
            loaded: false,
            title: String::new(),
            time_us: 0,
            duration_us: 0,
            paused: false,
            frozen: false,
            finished: false,
            wait_armed: false,
            awaiting: Vec::new(),
            held: Vec::new(),
            practice: "both".to_string(),
            split_pitch: DEFAULT_SPLIT,
            rate_permille: PLAY_RATE_UNITY,
            hear_song: false,
            monitor: false,
            score: 0,
            combo: 0,
            best_combo: 0,
            hits: 0,
            misses: 0,
            bpm: 0,
            beats_per_bar: 0,
            note_count: 0,
        })
}

/// Set play-session speed in permille (1000 = 1x), clamped to 0.25x..=2x.
/// Returns the applied value.
///
/// Slowing scales the time injected into the session, so the highway, wait gate
/// and scoring windows all stretch together — the chart is untouched, only the
/// wall-clock pace changes. The backing *recording* cannot follow without
/// resampling, so it is muted off-tempo and restored at 1x.
#[tauri::command]
pub fn play_set_rate(
    state: tauri::State<'_, PlayState>,
    audio: tauri::State<'_, AudioState>,
    rate_permille: u16,
) -> u16 {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    let Some(s) = guard.as_mut() else {
        return PLAY_RATE_UNITY;
    };
    let applied = s.set_rate(rate_permille);
    audio.set_backing_muted(applied != PLAY_RATE_UNITY);
    applied
}

/// Set the practiced hand: `"left"`, `"right"`, or anything else / null = both.
/// Returns the applied value (`"left"` / `"right"` / `"both"`).
#[tauri::command]
pub fn play_set_practice(state: tauri::State<'_, PlayState>, hand: Option<String>) -> String {
    let practice = match hand.as_deref() {
        Some("left") => Some(Hand::Left),
        Some("right") => Some(Hand::Right),
        _ => None,
    };
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    if let Some(s) = guard.as_mut() {
        s.set_practice(practice);
    }
    match practice {
        Some(Hand::Left) => "left",
        Some(Hand::Right) => "right",
        None => "both",
    }
    .to_string()
}

/// Set the left/right split pitch (0..=127). Returns the applied value.
#[tauri::command]
pub fn play_set_split(state: tauri::State<'_, PlayState>, pitch: u8) -> u8 {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    if let Some(s) = guard.as_mut() {
        s.set_split(pitch);
    }
    pitch
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

/// Toggle input-monitor: synthesise the player's own key presses (`n`). Returns
/// the new state; silences the synth when turning off.
#[tauri::command]
pub fn play_toggle_monitor(
    state: tauri::State<'_, PlayState>,
    audio: tauri::State<'_, AudioState>,
) -> bool {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    if let Some(s) = guard.as_mut() {
        let on = s.toggle_monitor();
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

/// Toggle a manual pause of the active take (`HostCommand::PlayTogglePause`).
/// Returns the new paused state (`false` when no session is active). The clock
/// freeze/thaw takes effect on the next tick; the backing audio follows the
/// clock via `tick_play`. The play-screen control/keys are M12-B (#232).
#[tauri::command]
pub fn play_toggle_pause(state: tauri::State<'_, PlayState>) -> bool {
    let mut guard = state.0.lock().expect("play state mutex poisoned");
    if let Some(s) = guard.as_mut() {
        s.toggle_pause()
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

    // Re-stamp live strikes with the current play-clock time before ingesting:
    // expected notes live in shifted play-clock time, and in wait mode the clock
    // freezes while device time keeps running, so the raw device timestamp would
    // never line up with an expected note (everything scored as a miss). Echoing
    // the player's notes on the Player bus (M14-C) is the input-monitor block
    // below, gated by `is_monitor()` — `audio.synth` is bound to that bus.
    let now = session.now_us();
    for &ev in midi_events {
        let stamped = match ev.kind {
            NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                NoteEvent::on(ev.note, velocity, now)
            }
            _ => NoteEvent::off(ev.note, now),
        };
        session.ingest(stamped);
    }
    // Input monitor: synthesise the player's own key presses so they hear
    // themselves. Independent of "hear the song" (the chart audition below).
    if session.is_monitor() {
        if let Some(synth) = &audio.synth {
            for &ev in midi_events {
                match ev.kind {
                    NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                        synth.note_on(ev.note, velocity);
                    }
                    _ => synth.note_off(ev.note),
                }
            }
        }
    }
    let frozen = session.advance(dt_us);

    // Route "hear the song" auditions through the **song** bus.
    let (need_on, need_off) = session.pending_song_triggers();
    if let Some(synth) = audio.bus(SynthBus::Song) {
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

    /// `status` reports the wait gate's demand alongside what is actually held —
    /// the pairing that makes a stuck take diagnosable over the socket.
    #[test]
    fn status_reports_awaiting_against_held() {
        let mut s = session();
        s.set_wait_mode(true);
        // `advance` polls the gate at the clock's *current* time and only then
        // advances it, so the freeze lands on the tick after the target passes.
        s.advance(target_us(0) + 1);
        s.advance(16_000);
        let st = s.status();
        assert!(st.loaded);
        assert!(st.wait_armed);
        assert!(st.frozen, "gate holds the clock at an unsatisfied step");
        assert_eq!(st.awaiting, vec![60], "the note the take is waiting for");
        assert!(st.held.is_empty(), "nothing pressed yet");
        assert_eq!(st.note_count, 4);
        assert_eq!(st.practice, "both");
        assert_eq!(st.rate_permille, PLAY_RATE_UNITY);

        // Press it: the gate is satisfied and the take moves on.
        s.ingest(on(60, s.now_us()));
        s.advance(16_000);
        let st = s.status();
        assert_eq!(st.held, vec![60], "status shows what is held");
        assert!(!st.frozen, "satisfied step releases the clock");
    }

    /// Regression: switching the practice hand **mid-take** must seek the rebuilt
    /// wait gate to the playhead, not restart it at the song's first note.
    /// Otherwise the gate freezes on a note already in the past and never accepts
    /// the note the player is meant to play now — the reported wait-mode lockup.
    #[test]
    fn switching_practice_mid_take_does_not_freeze_on_a_past_note() {
        // Two left-hand notes (48 @0, 50 @500ms — distinct pitches so we can tell
        // "stuck on the past one" from "waiting on the next one") around a
        // right-hand note (72 @250ms). Split defaults to 60, so 48/50 are left.
        let events = vec![
            on(48, 0),
            off(48, 200_000),
            on(72, 250_000),
            off(72, 450_000),
            on(50, 500_000),
            off(50, 700_000),
        ];
        let mut s = PlaySession::from_events("t".into(), &events);
        // Run forward (wait off) to between the two left notes — the "mid-take"
        // playhead — then switch to left-only practice.
        s.advance(target_us(1) + 100_000); // ~SHIFT+350ms: past 48@0, before 50@500
        s.set_practice(Some(Hand::Left));
        s.set_wait_mode(true);

        let st = s.status();
        assert!(
            st.awaiting != vec![48],
            "must not re-freeze on the already-passed first left note (was: {:?})",
            st.awaiting
        );

        // The upcoming left note still gates correctly: freeze on it when due,
        // release when played.
        let dt = (target_us(2) + 1).saturating_sub(s.now_us());
        s.advance(dt);
        s.advance(16_000);
        let st = s.status();
        assert!(st.frozen, "gate freezes on the current (due) left note");
        assert_eq!(st.awaiting, vec![50], "waits on the note at the playhead");
        s.ingest(on(50, s.now_us()));
        s.advance(16_000);
        assert!(
            !s.status().frozen,
            "playing the current left note proceeds — no lockup"
        );
    }

    /// Reading status must not perturb the take it observes — in particular it
    /// must not drain the judgment queue `live_state` owns, or an agent polling
    /// status would silently eat the webview's hit effects.
    #[test]
    fn status_is_free_of_side_effects() {
        let mut s = session();
        s.set_wait_mode(true);
        s.advance(target_us(0) + 1);
        s.advance(16_000);

        let before = s.status();
        for _ in 0..5 {
            let again = s.status();
            assert_eq!(
                again.time_us, before.time_us,
                "status must not advance time"
            );
            assert_eq!(again.awaiting, before.awaiting, "nor move the gate");
        }
        // The judgment queue survives repeated status reads.
        s.ingest(on(60, s.now_us()));
        s.advance(500_000);
        let _ = s.status();
        let ev = s.live_state();
        assert!(
            !ev.judgments.is_empty(),
            "status reads must leave judgments for the webview"
        );
    }

    /// A slowed take advances proportionally less per tick, leaving the chart
    /// untouched — the clock is what stretches.
    #[test]
    fn set_rate_scales_advance() {
        let mut s = session();
        assert_eq!(s.set_rate(500), 500);
        s.advance(1_000_000);
        assert_eq!(s.status().time_us, 500_000, "half speed, half the clock");

        // Out-of-range values clamp rather than erroring.
        assert_eq!(s.set_rate(1), PLAY_RATE_MIN);
        assert_eq!(s.set_rate(60_000), PLAY_RATE_MAX);
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

    /// Evidence + regression for the "wait mode pauses after the fact" report: a
    /// single oversized tick (a ~4 ms tick thread starved by a video decode / GC
    /// / lock spike) must NOT carry the clock past an unsatisfied wait-step's
    /// onset. Before the clamp, one big `advance` parked the clock wherever it
    /// overshot (here 1 s past a 200 ms note — well after the note ended), so the
    /// freeze landed after the fact. Now the tick is clamped to the onset and the
    /// freeze pins the note at the hit line.
    #[test]
    fn a_large_tick_freezes_at_the_note_onset_not_past_it() {
        let mut s = session();
        s.set_wait_mode(true);
        // One 1-second tick from a standstill: the first note (onset at SHIFT,
        // 200 ms long) is leapt over entirely. The clamp stops the clock dead on
        // the onset instead of at SHIFT + 1 s.
        s.advance(SHIFT + 1_000_000);
        assert_eq!(
            s.now_us(),
            target_us(0),
            "an armed tick must not overshoot the note onset"
        );
        // The next tick, now sitting exactly on the onset, freezes there.
        s.advance(250_000);
        assert!(s.live_state().frozen, "armed + unsatisfied → frozen");
        assert_eq!(
            s.now_us(),
            target_us(0),
            "the freeze pins the note at its onset, not past it"
        );
        // Playing the note releases the freeze and it advances normally.
        s.ingest(on(60, target_us(0)));
        s.advance(10_000);
        assert!(!s.live_state().frozen, "playing the awaited note resumes");
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

    /// Manual pause freezes the clock at its position and resumes from it.
    #[test]
    fn toggle_pause_freezes_then_resumes() {
        let mut s = session();
        s.advance(SHIFT);
        assert_eq!(s.now_us(), SHIFT);
        assert!(s.toggle_pause(), "toggling on reports paused");
        assert!(s.is_paused());
        // Frozen: wall-time deltas do not move the playhead, and the live event
        // reports frozen.
        s.advance(5_000_000);
        assert_eq!(s.now_us(), SHIFT, "clock frozen while paused");
        assert!(
            s.live_state().frozen,
            "paused reads as frozen for the webview"
        );
        // Resume from the same position.
        assert!(!s.toggle_pause(), "toggling off reports running");
        s.advance(250_000);
        assert_eq!(s.now_us(), SHIFT + 250_000, "resumes from frozen position");
    }

    /// Pause is independent of wait-mode: un-pausing while an armed step is
    /// unsatisfied leaves the clock frozen by wait-mode.
    #[test]
    fn toggle_pause_respects_active_wait_step_on_resume() {
        let mut s = session();
        s.set_wait_mode(true);
        s.advance(SHIFT); // parked on the first step, nothing held
        s.toggle_pause();
        s.advance(5_000_000);
        assert_eq!(s.now_us(), SHIFT);
        s.toggle_pause(); // un-pause, but wait-mode still holds the step
        s.advance(5_000_000);
        assert_eq!(s.now_us(), SHIFT, "wait-mode keeps the clock frozen");
        s.ingest(on(60, SHIFT));
        s.advance(250_000);
        assert_eq!(s.now_us(), SHIFT + 250_000);
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

    // ── "hear the song" default (issue #247) ────────────────────────────────

    /// The committed fixture bundle, whose `meta.json` declares no backing.
    fn midi_only_fixture() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("play-bundle")
    }

    /// Copy the fixture's `song.mid` into a temp bundle whose `meta.json`
    /// declares a backing track.
    fn backed_bundle(tmp: &tempfile::TempDir) -> PathBuf {
        let dir = tmp.path().to_path_buf();
        std::fs::copy(midi_only_fixture().join("song.mid"), dir.join("song.mid")).unwrap();
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: Some(rockcraft_core::BackingTrack {
                file: "backing.ogg".into(),
                audio_start_us: 0,
            }),
            grid: None,
            key: None,
            origin: Some(rockcraft_core::TrackOrigin::Composed),
            video: None,
            backgrounds: Vec::new(),
            hand_split: None,
            hand_overrides: Vec::new(),
            bar_starts: Vec::new(),
            version: 1,
        };
        std::fs::write(dir.join("meta.json"), meta.to_json()).unwrap();
        std::fs::write(dir.join("backing.ogg"), b"").unwrap();
        dir
    }

    // ── background images (M14-D) ───────────────────────────────────────────

    /// A bundle whose `meta.json` declares one animated background layer: a 2 s
    /// pan from centre to half a surface-width right.
    fn background_bundle(tmp: &tempfile::TempDir) -> PathBuf {
        use rockcraft_core::{Easing, Transform};
        let dir = tmp.path().to_path_buf();
        std::fs::copy(midi_only_fixture().join("song.mid"), dir.join("song.mid")).unwrap();
        let mut layer = BackgroundImage::new("bg-0", "background-0.png");
        layer.set_keyframe(0, Transform::IDENTITY, Easing::Linear);
        layer.set_keyframe(
            2_000_000,
            Transform::new(0.5, 0.0, 1.0, 0.0, 1.0),
            Easing::Linear,
        );
        let mut meta = RecordingMeta::new_midi_only("song.mid");
        meta.backgrounds = vec![layer];
        std::fs::write(dir.join("meta.json"), meta.to_json()).unwrap();
        std::fs::write(dir.join("background-0.png"), b"PNG").unwrap();
        dir
    }

    /// `play_load` hands the webview each layer's absolute path once; the moving
    /// part arrives per tick.
    #[test]
    fn play_info_lists_background_layers_with_absolute_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = background_bundle(&tmp);
        let s = load_session_from_dir(&dir).expect("load bundle");
        let info = s.info();
        assert_eq!(info.backgrounds.len(), 1);
        assert_eq!(info.backgrounds[0].id, "bg-0");
        assert_eq!(
            info.backgrounds[0].path,
            dir.join("background-0.png").to_string_lossy()
        );
    }

    /// The transform is evaluated by `core` against **song-content** time, i.e.
    /// after the pre-roll shift — so a keyframe authored at bar 1 does not fire
    /// during the empty lead-in.
    #[test]
    fn play_state_transforms_track_the_shifted_clock() {
        let tmp = tempfile::tempdir().unwrap();
        let mut s = load_session_from_dir(&background_bundle(&tmp)).expect("load bundle");
        s.toggle_pause(); // loads paused (start_paused); un-pause to "press Start"
        let shift = s.shift_us();

        // At t=0 the song content has not begun: the layer holds its first
        // keyframe.
        let st = s.live_state();
        assert_eq!(st.backgrounds.len(), 1);
        assert_eq!(st.backgrounds[0].id, "bg-0");
        assert!((st.backgrounds[0].transform.x).abs() < 1e-6);

        // One second of *content* in, the 2 s pan is half done.
        s.advance(shift + 1_000_000);
        let st = s.live_state();
        assert!(
            (st.backgrounds[0].transform.x - 0.25).abs() < 1e-4,
            "{:?}",
            st.backgrounds[0].transform
        );

        // Past the last keyframe the layer holds rather than flying off.
        s.advance(10_000_000);
        let st = s.live_state();
        assert!((st.backgrounds[0].transform.x - 0.5).abs() < 1e-5);
    }

    /// A piece with no background images sends nothing per tick.
    #[test]
    fn play_state_backgrounds_are_empty_without_layers() {
        let mut s = load_session_from_dir(&midi_only_fixture()).expect("load fixture bundle");
        assert!(s.info().backgrounds.is_empty());
        assert!(s.live_state().backgrounds.is_empty());
    }

    /// A MIDI-only piece would open silent without a live piano, so the synth
    /// audition defaults ON at load — and the webview sees it in `PlayInfo`.
    #[test]
    fn hear_song_defaults_on_for_midi_only_bundle() {
        let s = load_session_from_dir(&midi_only_fixture()).expect("load fixture bundle");
        assert!(
            s.info().hear_song,
            "a bundle with no backing track must audition itself"
        );
    }

    /// A piece with a real recording behind it doesn't need the synth doubling
    /// the melody, so the audition defaults OFF.
    #[test]
    fn hear_song_defaults_off_when_bundle_has_backing() {
        let tmp = tempfile::tempdir().unwrap();
        let s = load_session_from_dir(&backed_bundle(&tmp)).expect("load backed bundle");
        assert!(
            !s.info().hear_song,
            "a bundle with a backing track must not audition over it"
        );
    }

    /// The toggle still flips in both directions from whichever default applied,
    /// and turning it off still clears the audition bookkeeping.
    #[test]
    fn toggle_hear_song_works_from_either_default() {
        let mut s = load_session_from_dir(&midi_only_fixture()).unwrap();
        s.toggle_pause(); // loads paused (start_paused); un-pause to "press Start"
        s.advance(SHIFT);
        let (on_idx, _) = s.pending_song_triggers();
        assert_eq!(on_idx, vec![0], "MIDI-only starts auditioning");
        s.mark_song_fired(&on_idx, &[]);
        assert!(!s.toggle_hear_song(), "toggling off reports off");
        assert!(
            s.pending_song_triggers().0.is_empty(),
            "no triggers while off"
        );
        assert!(s.toggle_hear_song(), "toggling back on reports on");
        assert_eq!(
            s.pending_song_triggers().0,
            vec![0],
            "cleared bookkeeping re-arms the sounding span"
        );

        let tmp = tempfile::tempdir().unwrap();
        let mut backed = load_session_from_dir(&backed_bundle(&tmp)).unwrap();
        assert!(!backed.info().hear_song, "backed starts off");
        assert!(backed.toggle_hear_song(), "toggle lights it");
    }

    // ── per-note hit/near/miss feedback (M14-B, issue #258) ─────────────────

    /// A perfect take surfaces one `clear` judgment per note, at that note's
    /// lane, once its good-window has closed.
    #[test]
    fn perfect_take_surfaces_clear_feedback_per_note() {
        let mut s = session();
        for k in 0..4u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k)));
        }
        s.advance(target_us(3) + 200_000);
        let fx = s.live_state().judgments;
        assert_eq!(fx.len(), 4, "one judgment per note");
        for (k, f) in fx.iter().enumerate() {
            assert_eq!(f.note, PITCHES[k], "effect belongs at the note's lane");
            assert_eq!(f.level, "clear");
            assert_eq!(f.timing, "perfect");
            assert_eq!(f.error_us, 0);
            assert_eq!(f.time_us, target_us(k as u64));
        }
    }

    /// An off-time (but in-window) take reads `near`, not `clear` — the lesser
    /// effect is what tells the player their timing slipped.
    #[test]
    fn off_time_take_surfaces_near_feedback() {
        let mut s = session();
        for k in 0..4u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k) + 120_000));
        }
        s.advance(target_us(3) + 400_000);
        let fx = s.live_state().judgments;
        assert_eq!(fx.len(), 4);
        assert!(
            fx.iter().all(|f| f.level == "near" && f.timing == "late"),
            "120 ms late is a hit, but not a clear one: {fx:?}"
        );
        assert!(fx.iter().all(|f| f.error_us == 120_000));
    }

    /// A note never struck reads `subtle`/`miss` — still acknowledged, quietly.
    #[test]
    fn missed_note_surfaces_subtle_feedback() {
        let mut s = session();
        for k in 0..3u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k)));
        }
        s.advance(target_us(3) + 200_000);
        let fx = s.live_state().judgments;
        let miss: Vec<_> = fx.iter().filter(|f| f.timing == "miss").collect();
        assert_eq!(miss.len(), 1);
        assert_eq!(miss[0].note, PITCHES[3], "the un-played note's lane");
        assert_eq!(miss[0].level, "subtle");
    }

    /// Judgments are one-shot: they arrive on the tick their window closed and
    /// never repeat, so the webview can spawn an effect per entry blindly.
    #[test]
    fn feedback_is_delivered_once_and_only_when_due() {
        let mut s = session();
        s.ingest(on(PITCHES[0], target_us(0)));
        // Before the first note's good-window closes, nothing is final yet.
        s.advance(target_us(0));
        assert!(
            s.live_state().judgments.is_empty(),
            "no judgment before the window closes"
        );
        // Past the window: exactly the first note.
        s.advance(ScoreConfig::default().good_us + 1);
        let fx = s.live_state().judgments;
        assert_eq!(fx.len(), 1);
        assert_eq!(fx[0].note, PITCHES[0]);
        // Reading again yields nothing — the queue was drained.
        assert!(s.live_state().judgments.is_empty(), "drained, not repeated");
    }

    /// The live hit/miss counters and the queued feedback describe the same
    /// notes: every judged note is counted exactly once in both.
    #[test]
    fn feedback_count_matches_live_counters() {
        let mut s = session();
        for k in 0..3u64 {
            s.ingest(on(PITCHES[k as usize], target_us(k)));
        }
        s.advance(target_us(3) + 200_000);
        let live = s.live_state();
        assert_eq!(live.judgments.len(), live.hits + live.misses);
        let clear = live.judgments.iter().filter(|f| f.level == "clear").count();
        assert_eq!(clear, live.hits);
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

    // ── per-note hand assignment (M14-E) ─────────────────────────────────────

    /// Mark the fixture's first note (C4 = 60, right hand under the default
    /// split) as a **left**-hand crossover.
    fn crossover_session() -> PlaySession {
        let overrides = [HandOverride {
            pitch: 60,
            // The pre-shift position — what the editor saved.
            start_us: 0,
            hand: Hand::Left,
        }];
        PlaySession::from_events("test".into(), &four_note_events())
            .with_hands(&overrides, DEFAULT_SPLIT)
    }

    #[test]
    fn with_hands_matches_the_pre_shift_position() {
        let s = crossover_session();
        let first = s.spans.first().expect("fixture has notes");
        assert_eq!(first.note, 60);
        assert_eq!(first.start_us, SHIFT, "span is shifted by the pre-roll");
        assert_eq!(
            first.hand,
            Some(Hand::Left),
            "the override is keyed by the ORIGINAL start, not the shifted one"
        );
        // Every other note keeps the split default.
        assert!(s.spans[1..].iter().all(|s| s.hand.is_none()));
        assert_eq!(first.effective_hand(DEFAULT_SPLIT), Hand::Left);
        assert_eq!(s.spans[1].effective_hand(DEFAULT_SPLIT), Hand::Right);
    }

    #[test]
    fn overridden_note_is_practised_and_scored_on_its_marked_hand() {
        let mut s = crossover_session();
        s.set_practice(Some(Hand::Left));
        s.set_wait_mode(true);

        // The left hand owns exactly the overridden note — the gate waits for it
        // even though its pitch sits on the right side of the split.
        assert_eq!(
            expected_notes_for(&s.spans, s.practice, s.split_pitch).len(),
            1,
            "only the crossover is scored while practising the left hand"
        );
        s.advance(target_us(0) + 1);
        s.advance(16_000);
        assert_eq!(s.status().awaiting, vec![60]);

        // Playing it lands as a hit.
        s.ingest(on(60, s.now_us()));
        s.advance(16_000);
        assert!(!s.status().frozen);
        s.advance(target_us(3) + 200_000);
        assert_eq!(s.live_state().hits, 1);
    }

    #[test]
    fn the_other_hand_auto_plays_the_overridden_note() {
        let mut s = crossover_session();
        s.set_practice(Some(Hand::Right));

        // Practising the right hand, the crossover is accompaniment: it is not
        // scored, and it auto-sounds.
        let expected = expected_notes_for(&s.spans, s.practice, s.split_pitch);
        assert_eq!(expected.len(), 3, "the crossover is not the right hand's");
        s.advance(target_us(0) + 1);
        let (need_on, _) = s.pending_song_triggers();
        assert_eq!(
            need_on
                .iter()
                .filter_map(|&i| s.span_note(i))
                .collect::<Vec<_>>(),
            vec![60],
            "the left-hand crossover auto-plays under right-hand practice"
        );
        // ...and never counts as a miss.
        s.advance(target_us(3) + 200_000);
        assert_eq!(s.live_state().misses, 3, "only the three right-hand notes");
    }

    #[test]
    fn a_bundle_with_no_overrides_behaves_exactly_as_before() {
        let mut plain = PlaySession::from_events("test".into(), &four_note_events())
            .with_hands(&[], DEFAULT_SPLIT);
        let mut untouched = session();
        for s in [&mut plain, &mut untouched] {
            s.set_practice(Some(Hand::Right));
        }
        assert_eq!(
            expected_notes_for(&plain.spans, plain.practice, plain.split_pitch).len(),
            expected_notes_for(&untouched.spans, untouched.practice, untouched.split_pitch).len(),
        );
        // All four are the right hand at the default split — the pitch-only rule.
        assert_eq!(
            expected_notes_for(&plain.spans, plain.practice, plain.split_pitch).len(),
            4
        );
        assert!(plain.spans.iter().all(|s| s.hand.is_none()));
    }

    #[test]
    fn a_low_split_is_honoured_without_any_overrides() {
        // A piece that declares its own split line: at 66, 60/62/64/65 all read
        // as the left hand even though nothing is overridden.
        let mut s =
            PlaySession::from_events("test".into(), &four_note_events()).with_hands(&[], 66);
        s.set_practice(Some(Hand::Left));
        assert_eq!(
            expected_notes_for(&s.spans, s.practice, s.split_pitch).len(),
            4
        );
        assert_eq!(s.status().split_pitch, 66);
    }

    #[test]
    fn play_info_carries_the_effective_hand() {
        let s = crossover_session();
        let info = s.info();
        assert_eq!(info.notes[0].hand, Hand::Left, "override wins");
        assert_eq!(info.notes[1].hand, Hand::Right, "split rule for the rest");
    }

    #[test]
    fn retuning_the_split_live_leaves_overrides_alone() {
        // `play_set_split` re-classifies the un-overridden notes only.
        let mut s = crossover_session();
        s.set_split(70); // everything below 70 is now the left hand
        assert_eq!(s.spans[0].effective_hand(70), Hand::Left, "still pinned");
        assert_eq!(s.spans[1].effective_hand(70), Hand::Left, "re-classified");
        s.set_split(21); // ...and now everything reads right — except the pin
        assert_eq!(s.spans[0].effective_hand(21), Hand::Left);
        assert_eq!(s.spans[1].effective_hand(21), Hand::Right);
    }
}
