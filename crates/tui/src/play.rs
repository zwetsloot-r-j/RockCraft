//! Play screen: a falling-note highway above the keyboard. Loads a `.mid`,
//! scrolls its notes down to the keyboard line on a playback clock, and lights
//! the player's live keys over it (play-along; scoring is a later task).
//!
//! **Practice loop (M17-B).** `[` / `]` mark bars and `l` loops them:
//! count-in → demo (the app plays the passage) → count-in → your turn → …,
//! driven by `core`'s [`PracticeLoop`]. The same model as the desktop app's
//! (`specs/M17-A-play-practice-loop.md`), minus scoring, which the TUI lacks.

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
use rockcraft_audio::{
    BackingHandle, BackingOut, DecodedTrack, SynthHandle, TrackLoader, TrackStatus,
};
use rockcraft_core::{
    backing_position_us, hand::hand_of_pitch_value, score, BarMap, DriftGuard, ExpectedNote, Gain,
    GateState, Grid, Hand, HandOverride, LoopPhase, LoopStep, MidiNote, NoteEvent, NoteEventKind,
    NoteJudgment, Pass, PlayClock, PracticeLoop, ScoreConfig, Summary, SustainEvent, SynthBus,
    Timing, Velocity, WaitGate, DEFAULT_SPLIT,
};
use rockcraft_midi::smf_bytes_to_events;

use crate::highway::{
    build_spans, cells_of_extent, grid_rows, line_cell, lower_block, next_per_lane,
    song_duration_us, span_extent8, step_to_sixteenth, CellFill, NoteSpan, SUB,
};
use crate::keyboard::{black_key_col, is_black_key, white_index, HeldNotes, Scale};
use crate::palette::{note_color, ColorMode, Rgb, BACKGROUND};
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

/// The practice loop's count-in click, on the song bus — the same click as the
/// editor metronome and the desktop app's count-in.
const CLICK_NOTE: u8 = 76;
const CLICK_VEL_ACCENT: u8 = 110;
const CLICK_VEL_NORMAL: u8 = 80;
/// How long a click rings before it is released.
const CLICK_DUR_US: u64 = 50_000;

/// How long the song voice and the backing take to fade out when the
/// transport stops (a wait-mode freeze or a pause) — soft, not a dead stop.
/// The same as the desktop app's.
const FREEZE_FADE: Duration = Duration::from_millis(250);
/// How long they take to come back when it resumes: just enough to avoid a
/// click, short enough to feel instant.
const THAW_FADE: Duration = Duration::from_millis(15);

/// Practice speed in permille: 1000 is the piece's own tempo.
pub const PLAY_RATE_UNITY: u16 = 1000;
/// Slowest practice speed a host command may set (0.25×).
pub const PLAY_RATE_MIN: u16 = 250;
/// Fastest practice speed a host command may set (2×).
pub const PLAY_RATE_MAX: u16 = 2000;
/// The speeds `-` / `=` step through — the desktop's presets.
pub const RATE_STEPS: [u16; 5] = [500, 625, 750, 875, 1000];

/// A backing audio track attached to a bundle, plus the file position that
/// lines up with recording time 0 (`audio_start_us`, from Task C).
struct Backing {
    path: PathBuf,
    audio_start_us: i64,
}

pub struct PlayScreen {
    spans: Vec<NoteSpan>,
    /// The hand playing each span (parallel to `spans`): the piece's per-note
    /// override, else its split line. Drives the "hands" colouring.
    hands: Vec<Hand>,
    /// How the highway colours its notes (`c` cycles it).
    color_mode: ColorMode,
    /// How the highway scrolls (`v` toggles it).
    scroll_mode: ScrollMode,
    /// The piece's tempo map (bar downbeats, play-clock µs) for the highway's
    /// bar lines; empty → uniform `bar_us` bars from `bar_origin_us`.
    bar_starts_us: Vec<u64>,
    /// Uniform bar length (the grid's, or 120 BPM 4/4 without one).
    bar_us: u64,
    /// Play-clock time of the uniform grid's first downbeat.
    bar_origin_us: u64,
    /// Beats per bar, for the beat lines.
    beats_per_bar: u8,
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
    /// The **player** voice: the notes coming off the piano, echoed live.
    synth: Option<SynthHandle>,
    /// The **song** voice: "hear the song". Same synth, its own MIDI channel,
    /// so instrument and level are independent of the player's (M14-C).
    song_synth: Option<SynthHandle>,
    /// The backing track's level. Applied when the track arms (a fresh sink
    /// starts at unity) and on the live handle when it changes mid-take.
    backing_gain: Gain,
    /// Whole-song forward shift applied to the spans; the clock value at which
    /// the first note's lead-in ends and the backing track should begin.
    /// Equals `song_shift_us(first_note_us, PRE_ROLL_US, LEAD_US)`.
    shift_us: u64,
    /// Backing track to sync, if the loaded bundle has one.
    backing: Option<Backing>,
    /// Live playback handle once the backing track has started; `None` until the
    /// clock reaches `shift_us` (and again after `restart` re-arms it).
    backing_handle: Option<BackingHandle>,
    /// The backing track decoding (then decoded) in the background. Started on
    /// the first tick, so it is usually ready by the end of the lead-in; kept
    /// across `restart` so a replay starts instantly.
    backing_track: Option<TrackLoader>,
    /// Keeps the playing backing in step with the clock: an audio underrun
    /// delays the stream for good, so it is re-seeked when it drifts.
    backing_drift: DriftGuard,
    /// How many times the backing was re-seeked this take (status line).
    backing_resyncs: u32,
    /// Whether the song voice and the backing are faded out for a stopped
    /// transport (see [`sync_parked_audio`](Self::sync_parked_audio)).
    audio_parked: bool,
    /// Whether the "hear the song" feature is active.
    hear_song: bool,
    /// Manual pause (the `Space` key / `HostCommand::PlayTogglePause`). Freezes
    /// the clock + backing independently of wait-mode; while set the highway,
    /// playhead, and scoring clock hold their position.
    paused: bool,
    /// Span indices for which we have already sent note_on to the song synth.
    song_on_fired: HashSet<usize>,
    /// Span indices for which we have already sent note_off to the song synth.
    song_off_fired: HashSet<usize>,
    /// The player's wait-mode setting. Outside a loop the gate is armed exactly
    /// when this is; inside one only during *your turn*.
    wait_mode: bool,
    /// Marked loop bars (`[` / `]`), 0-based.
    loop_marks: (Option<u64>, Option<u64>),
    /// The running practice loop, if any.
    practice_loop: Option<RunningLoop>,
    /// The practised hand (`h`, M18-C): `None` = both hands. The app plays the
    /// other hand, and only this hand's notes are waited on.
    practice: Option<Hand>,
    /// When the sounding count-in click is released (play-clock µs).
    click_off_at: Option<u64>,
    /// Practice speed in permille ([`PLAY_RATE_UNITY`] = 1×). Scales the time
    /// fed into the clock, so the highway, wait gate and song stretch together.
    rate_permille: u16,
    /// Timing windows for judging strikes.
    cfg: ScoreConfig,
    /// The take's strikes (note-ons), stamped with the play clock.
    played: Vec<NoteEvent>,
    /// Spans already judged: a span is final once the clock passes the end of
    /// its good window.
    scored: HashSet<usize>,
    /// Live figures over the judged spans (the running loop's pass while one
    /// runs), equal to the final report once every span is judged.
    score: u64,
    combo: u32,
    best_combo: u32,
    live_hits: usize,
    live_misses: usize,
    /// The last finished *your turn* pass of the practice loop.
    last_pass: Option<PassSummary>,
    /// The take's summary, frozen when the song ends (the summary screen).
    summary: Option<PlaySummary>,
}

/// A running practice loop: the phase machine plus the bars it covers.
struct RunningLoop {
    lp: PracticeLoop,
    first_bar: u64,
    last_bar: u64,
    /// The practised hand as of the last phase boundary: a change mid-loop
    /// takes effect at the next one (M18-C).
    practice: Option<Hand>,
    /// Strikes of the current *your turn* pass (its count-in included, so an
    /// early first note counts). Kept apart from the whole take's, so stopping
    /// the loop restores the take exactly.
    played: Vec<NoteEvent>,
    /// Loop spans already judged this pass.
    scored: HashSet<usize>,
}

impl RunningLoop {
    /// Does `span` start inside the loop?
    fn contains(&self, span: &NoteSpan) -> bool {
        (self.lp.start_us()..self.lp.end_us()).contains(&span.start_us)
    }
}

/// The practice loop as the status line and the control socket see it. Bars
/// are 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopView {
    pub first_bar: u64,
    pub last_bar: u64,
    /// `[start_us, end_us)` of the marked bars, in play-clock µs.
    pub start_us: u64,
    pub end_us: u64,
    /// Whether the loop is running (false: only marked).
    pub running: bool,
    /// The phase while running.
    pub phase: Option<LoopPhase>,
    /// 1-based pass number (0 unless running).
    pub pass: u32,
    /// The last finished *your turn* pass, if any.
    pub last_pass: Option<PassSummary>,
}

/// One finished *your turn* pass of the practice loop — the desktop's
/// `PassSummary`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PassSummary {
    /// 1-based pass number.
    pub pass: u32,
    pub hits: usize,
    pub misses: usize,
    /// Accuracy in basis points (0..=10000).
    pub accuracy_bp: u32,
}

/// The end-of-take summary (the summary screen, `play_finish`) — the
/// desktop's `PlaySummary`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PlaySummary {
    pub total_expected: usize,
    pub hits: usize,
    pub misses: usize,
    pub extras: usize,
    pub perfect: usize,
    pub early: usize,
    pub late: usize,
    /// Accuracy in basis points (0..=10000); divide by 100 for a percentage.
    pub accuracy_bp: u32,
    pub best_combo: u32,
    pub score: u64,
}

/// The live take as `play_status` reports it — the desktop's
/// `PlayStatusView` fields the TUI has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlayStatus {
    pub title: String,
    pub time_us: u64,
    pub duration_us: u64,
    pub paused: bool,
    /// Clock held — by the manual pause or an unsatisfied wait step.
    pub frozen: bool,
    pub finished: bool,
    pub wait_armed: bool,
    /// Pitches the wait gate is holding for; empty unless it holds.
    pub awaiting: Vec<u8>,
    pub held: Vec<u8>,
    pub rate_permille: u16,
    pub hear_song: bool,
    pub score: u64,
    pub combo: u32,
    pub best_combo: u32,
    pub hits: usize,
    pub misses: usize,
    pub beats_per_bar: u8,
    pub note_count: usize,
    pub bar: u64,
    pub practice_loop: Option<LoopView>,
}

/// Accuracy as basis points, the JSON-exact form both frontends report.
fn accuracy_bp(summary: &Summary) -> u32 {
    (summary.accuracy() * 10_000.0).round() as u32
}

/// Per-note score award, the desktop's economy: perfect = 100 + 2·combo,
/// good (early/late) = 50 + combo. A miss resets the combo.
fn award(perfect: bool, combo: u32) -> u64 {
    if perfect {
        100 + combo as u64 * 2
    } else {
        50 + combo as u64
    }
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
        let hands = spans
            .iter()
            .map(|s| hand_of_pitch_value(s.note, DEFAULT_SPLIT))
            .collect();

        let song_synth = synth.as_ref().map(|s| s.for_bus(SynthBus::Song));

        Ok(Self {
            spans,
            hands,
            color_mode: ColorMode::default(),
            scroll_mode: ScrollMode::default(),
            bar_starts_us: Vec::new(),
            bar_us: Grid::default_120().bar_us(),
            bar_origin_us: offset,
            beats_per_bar: Grid::default_120().time_sig.beats_per_bar,
            duration_us,
            held: HeldNotes::new(),
            clock: PlayClock::new(),
            last_tick: None,
            wait,
            title,
            finished_pause_us: LEAD_US,
            synth,
            song_synth,
            backing_gain: Gain::UNITY,
            shift_us: offset,
            backing: None,
            backing_handle: None,
            backing_track: None,
            backing_drift: DriftGuard::new(),
            backing_resyncs: 0,
            audio_parked: false,
            hear_song: false,
            paused: false,
            song_on_fired: HashSet::new(),
            song_off_fired: HashSet::new(),
            wait_mode: false,
            loop_marks: (None, None),
            practice_loop: None,
            practice: None,
            click_off_at: None,
            rate_permille: PLAY_RATE_UNITY,
            cfg: ScoreConfig::default(),
            played: Vec::new(),
            scored: HashSet::new(),
            score: 0,
            combo: 0,
            best_combo: 0,
            live_hits: 0,
            live_misses: 0,
            last_pass: None,
            summary: None,
        })
    }

    /// Start with the "hear the song" audition on or off. Bundle loading passes
    /// `meta.backing.is_none()` here (#247): a MIDI-only piece auditions itself
    /// so it isn't silent without a live piano, while a piece with a backing
    /// track leaves it off so the synth doesn't double the recording. The `m`
    /// key still toggles it at runtime either way. Scoring keys off live MIDI
    /// timestamps regardless, never this audio.
    pub fn with_hear_song(mut self, on: bool) -> Self {
        self.hear_song = on;
        self
    }

    /// Assign each note its hand from the piece's split line and per-note
    /// overrides (`meta.json`, M14-E). Overrides are keyed by the note's
    /// position in the *file*, before the lead-in shift.
    pub fn with_hands(mut self, split: u8, overrides: &[HandOverride]) -> Self {
        let shift = self.shift_us;
        self.hands = self
            .spans
            .iter()
            .map(|s| {
                let file_start = s.start_us.saturating_sub(shift);
                overrides
                    .iter()
                    .find(|o| o.pitch == s.note && o.start_us == file_start)
                    .map(|o| o.hand)
                    .unwrap_or_else(|| hand_of_pitch_value(s.note, split))
            })
            .collect();
        self
    }

    /// Adopt the piece's grid (tempo + metre) and tempo map (`meta.grid`,
    /// `meta.bar_starts`, in file µs) for the highway's bar and beat lines,
    /// shifted into play-clock time alongside the notes so they stay on them.
    pub fn with_grid(mut self, grid: Option<Grid>, bar_starts: &[u64]) -> Self {
        let grid = grid.unwrap_or_else(Grid::default_120);
        self.bar_us = grid.bar_us().max(1);
        self.bar_origin_us = grid.origin_us + self.shift_us;
        self.beats_per_bar = grid.time_sig.beats_per_bar.max(1);
        self.bar_starts_us = bar_starts.iter().map(|b| b + self.shift_us).collect();
        self
    }

    /// The bar lookup for the highway grid.
    fn bar_map(&self) -> BarMap<'_> {
        BarMap::new(&self.bar_starts_us, self.bar_us).with_origin(self.bar_origin_us)
    }

    /// The hand assigned to each note, in span order.
    pub fn hands(&self) -> &[Hand] {
        &self.hands
    }

    /// How the highway colours its notes.
    pub fn color_mode(&self) -> ColorMode {
        self.color_mode
    }

    /// How the highway scrolls.
    pub fn scroll_mode(&self) -> ScrollMode {
        self.scroll_mode
    }

    /// Set how the highway scrolls.
    pub fn set_scroll_mode(&mut self, mode: ScrollMode) {
        self.scroll_mode = mode;
    }

    /// Set how the highway colours its notes.
    pub fn set_color_mode(&mut self, mode: ColorMode) {
        self.color_mode = mode;
    }

    /// Attach a backing audio track from the loaded bundle. `audio_start_us` is
    /// the position in the file that lines up with recording time 0 (Task C).
    /// Playback is armed lazily: it begins when the clock reaches `shift_us`.
    pub fn with_backing(mut self, path: PathBuf, audio_start_us: i64) -> Self {
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
        // A restart un-pauses: the clock runs again from the top.
        self.paused = false;
        // …and leaves a running loop (its marks stay).
        self.practice_loop = None;
        self.click_off_at = None;
        // A fresh take: nothing played, nothing judged.
        self.played.clear();
        self.scored.clear();
        self.last_pass = None;
        self.summary = None;
        self.recompute_live();
        // Rebuild the wait tracker from the top, keeping wait-mode armed or not.
        self.reset_full_gate(0);
        self.song_on_fired.clear();
        self.song_off_fired.clear();
        if let Some(s) = &self.synth {
            s.all_off();
        }
        if let Some(h) = self.backing_handle.take() {
            h.stop();
        }
        self.backing_resyncs = 0;
        // `all_off` above brought every synth bus back to full level.
        self.audio_parked = false;
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
    /// backing track, no audio output (`out`), or it is already playing.
    ///
    /// Never blocks: the file decodes on a background thread, kicked off on the
    /// first call. Should it still be decoding when the lead-in ends, the track
    /// starts once it is ready — at the position the clock has reached by then,
    /// so it joins in step rather than trailing the notes.
    pub fn tick_backing(&mut self, out: Option<&BackingOut>) {
        // Silent on the summary screen (`restart` brings it back).
        if self.summary.is_some() {
            return;
        }
        self.sync_parked_audio();
        if self.backing_handle.is_some() {
            self.keep_backing_in_step();
            return;
        }
        let (Some(out), Some(b)) = (out, &self.backing) else {
            return;
        };
        let loader = self
            .backing_track
            .get_or_insert_with(|| DecodedTrack::load_in_background(&b.path));
        let track = match loader.poll() {
            TrackStatus::Loading => return,
            TrackStatus::Ready(t) => t.clone(),
            // A missing/undecodable file: drop the track rather than retry
            // every frame; the song still plays without it.
            TrackStatus::Failed(_) => {
                self.backing = None;
                self.backing_track = None;
                return;
            }
        };
        let Some(pos_us) = self.backing_target_us(self.now_us()) else {
            return;
        };
        match out.play_at(&track, Duration::from_micros(pos_us)) {
            Ok(h) => {
                // If the transport is stopped at the moment the backing arms,
                // start it silent and held; the thaw re-seeks and fades it in.
                if self.audio_parked {
                    h.fade_out(Duration::ZERO);
                }
                // A fresh sink starts at unity — carry the fader onto it.
                h.set_gain(self.applied_backing_gain());
                self.backing_handle = Some(h);
                self.backing_drift.restart(self.now_us());
            }
            Err(_) => {
                self.backing = None;
                self.backing_track = None;
            }
        }
    }

    /// Re-seek the playing backing when it has drifted from the clock (see
    /// `core::DriftGuard`). Only while both run: a stopped transport fades the
    /// backing out and holds it, and the guard would read that as drift.
    fn keep_backing_in_step(&mut self) {
        let now = self.now_us();
        let (Some(h), Some(target)) = (&self.backing_handle, self.backing_target_us(now)) else {
            return;
        };
        // Muted off-tempo, it runs at full speed ahead of the clock; the
        // return to 1× restarts it in step instead.
        if !self.clock.is_running() || self.audio_parked || self.is_off_tempo() {
            return;
        }
        let position = h.position().as_micros() as u64;
        if let Some(seek) = self.backing_drift.check(now, target, position) {
            h.seek(Duration::from_micros(seek));
            self.backing_resyncs += 1;
        }
    }

    /// Whether the transport is held at the clock's *current* position: paused,
    /// or parked on an unsatisfied wait step. Unlike the state going into
    /// [`advance`](Self::advance), this is already true on the very tick the
    /// clock lands on the step's onset. Re-polls the gate — an idempotent read.
    pub fn parked(&mut self) -> bool {
        self.paused || self.wait.poll(self.clock.now_us()) == GateState::Frozen
    }

    /// Fade the song voice and the backing out when the transport stops, and
    /// back in when it resumes — the desktop app's freeze fade. Acts only on a
    /// change, so it is cheap to call every tick.
    ///
    /// Keyed off [`parked`](Self::parked) rather than the clock: in legato
    /// music the notes before a wait step end exactly at its onset, on the very
    /// tick the clock lands there, and must fade with the rest rather than be
    /// released a tick before the freeze is seen. The backing keeps running
    /// (silent) through its fade, so the thaw re-seeks it to the clock.
    fn sync_parked_audio(&mut self) {
        let parked = self.parked();
        if parked == self.audio_parked {
            return;
        }
        self.audio_parked = parked;
        if parked {
            if let Some(s) = &self.song_synth {
                s.fade_out(FREEZE_FADE);
            }
            if let Some(h) = &self.backing_handle {
                h.fade_out(FREEZE_FADE);
            }
        } else {
            if let Some(s) = &self.song_synth {
                s.fade_in(THAW_FADE);
            }
            let now = self.now_us();
            if let (Some(h), Some(target)) = (&self.backing_handle, self.backing_target_us(now)) {
                h.seek(Duration::from_micros(target));
                h.fade_in(THAW_FADE);
                self.backing_drift.restart(now);
            }
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
        // The summary screen holds the finished take still until `r`.
        if self.summary.is_some() {
            return;
        }
        // Practice speed scales the time entering the clock, not the chart, so
        // the gate and the song stretch with it (the desktop does the same).
        let dt_us = dt_us * self.rate_permille as u64 / PLAY_RATE_UNITY as u64;
        // The loop wraps BEFORE the gate poll, so a step at exactly `end_us`
        // (the next bar's first note) never freezes the loop.
        self.tick_loop();
        let held: BTreeSet<u8> = self.held.iter().collect();
        self.wait.set_held(held);
        let wait_frozen = self.wait.poll(self.clock.now_us()) == GateState::Frozen;
        // A manual pause freezes the transport just like an unsatisfied wait step.
        let frozen = self.paused || wait_frozen;
        // The audio follows in `sync_parked_audio` (a fade, not a dead stop).
        if frozen && self.clock.is_running() {
            self.clock.pause();
        } else if !frozen && !self.clock.is_running() {
            self.clock.resume();
        }
        // Land exactly on the next wait step's onset rather than past it, so
        // the freeze is seen on that same tick — before the song releases the
        // notes ending there — and the note sits on the keyboard line.
        let mut step_us = dt_us;
        if !frozen && self.wait.is_armed() {
            if let Some(next) = self.wait.next_step_time() {
                let now = self.clock.now_us();
                if next > now {
                    step_us = step_us.min(next - now);
                }
            }
        }
        // Likewise the loop's next boundary (loop start after a count-in, loop
        // end after a pass), so phases switch on the downbeat.
        if !frozen {
            if let Some(boundary) = self.loop_boundary() {
                let now = self.clock.now_us();
                if boundary > now {
                    step_us = step_us.min(boundary - now);
                }
            }
        }
        let prev_us = self.clock.now_us();
        self.clock.advance(step_us);
        self.loop_clicks(prev_us, self.clock.now_us());
        self.tick_loop();
        self.score_due();
        // The song is over: freeze its summary for the summary screen, and
        // stop a backing that runs on past the last note.
        if self.is_finished() {
            self.summary = Some(self.finish());
            if let Some(h) = self.backing_handle.take() {
                h.stop();
            }
        }
    }

    /// Toggle a manual pause of the play session (the `Space` key /
    /// `HostCommand::PlayTogglePause`). Freezes the clock + backing when
    /// pausing and thaws them when resuming, reusing the same freeze/thaw
    /// machinery wait-mode uses — so the highway, playhead, and scoring clock
    /// all hold their position and continue from it (no jump, no missed-note
    /// storm).
    pub fn toggle_pause(&mut self) {
        self.paused = !self.paused;
        if self.paused {
            // Freeze now so audio fades on the keystroke, not a frame later.
            if self.clock.is_running() {
                self.clock.pause();
            }
        } else {
            // Resume immediately — unless wait-mode is currently holding an
            // unsatisfied step, in which case the next `advance` re-freezes.
            let held: BTreeSet<u8> = self.held.iter().collect();
            self.wait.set_held(held);
            let wait_frozen = self.wait.poll(self.clock.now_us()) == GateState::Frozen;
            if !wait_frozen && !self.clock.is_running() {
                self.clock.resume();
            }
        }
        self.sync_parked_audio();
    }

    /// Is the session manually paused? (For the play HUD / control snapshot.)
    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// The practice speed in permille ([`PLAY_RATE_UNITY`] = 1×).
    pub fn rate_permille(&self) -> u16 {
        self.rate_permille
    }

    /// Set the practice speed in permille, clamped to
    /// [`PLAY_RATE_MIN`]..=[`PLAY_RATE_MAX`]; returns the applied value. Off
    /// 1× the backing is muted (see [`applied_backing_gain`](Self::applied_backing_gain));
    /// back at 1× it is restarted at the clock's position — the muted track ran
    /// on at full speed — by dropping it for [`tick_backing`](Self::tick_backing)
    /// to re-arm, the same path a loop jump takes.
    pub fn set_rate(&mut self, rate_permille: u16) -> u16 {
        let was_off_tempo = self.is_off_tempo();
        self.rate_permille = rate_permille.clamp(PLAY_RATE_MIN, PLAY_RATE_MAX);
        if was_off_tempo && !self.is_off_tempo() {
            if let Some(h) = self.backing_handle.take() {
                h.stop();
            }
        } else if let Some(h) = &self.backing_handle {
            h.set_gain(self.applied_backing_gain());
        }
        self.rate_permille
    }

    /// Step the practice speed through [`RATE_STEPS`] (the `-` / `=` keys):
    /// one step slower when `faster` is false, one faster otherwise, never past
    /// 1×. From a speed off the steps (set over the socket) it moves to the
    /// nearest step in that direction. Returns the applied value.
    pub fn nudge_rate(&mut self, faster: bool) -> u16 {
        let r = self.rate_permille;
        let next = if faster {
            RATE_STEPS.iter().copied().find(|&s| s > r)
        } else {
            RATE_STEPS.iter().copied().rev().find(|&s| s < r)
        };
        // Past either end of the steps (slower than 0.5×, or 1× and up), stay.
        self.set_rate(next.unwrap_or(r))
    }

    /// Whether the take runs at anything but the piece's own tempo.
    fn is_off_tempo(&self) -> bool {
        self.rate_permille != PLAY_RATE_UNITY
    }

    /// Toggle note-by-note wait-mode (the `w` key / `Action::ToggleWaitMode`).
    pub fn toggle_wait_mode(&mut self) {
        self.set_wait_mode(!self.wait_mode);
    }

    /// Set wait-mode on/off (`Action::SetWaitMode`). Turning it off immediately
    /// un-freezes the clock and backing so play resumes without waiting a frame.
    pub fn set_wait_mode(&mut self, on: bool) {
        self.wait_mode = on;
        // In a loop the gate only runs during *your turn*; the next phase
        // boundary picks the setting up otherwise.
        let gated = self
            .practice_loop
            .as_ref()
            .is_none_or(|rl| rl.lp.phase() == LoopPhase::YourTurn);
        self.wait.set_armed(on && gated);
        if !on && !self.paused && !self.clock.is_running() {
            self.clock.resume();
        }
        self.sync_parked_audio();
    }

    /// Is wait-mode armed? (For the status line / control snapshot.)
    pub fn is_wait_mode(&self) -> bool {
        self.wait_mode
    }

    /// The notes the player must hold to un-freeze, if currently waiting.
    fn awaiting_notes(&self) -> Option<Vec<u8>> {
        self.wait.awaiting().map(|step| step.notes.clone())
    }

    /// Whether the backing track is currently audible (for the status line).
    fn backing_playing(&self) -> bool {
        self.backing_handle.is_some()
    }

    /// Forward a live `NoteEvent` to the held-key tracker, scoring and the
    /// synth.
    pub fn ingest(&mut self, ev: NoteEvent) {
        self.track_held(ev);
        if let Some(s) = &self.synth {
            s.apply(&ev);
        }
    }

    /// Forward a sustain-pedal change to the player's synth voice, so your
    /// own notes ring on while it is held. Scoring never sees the pedal.
    pub fn apply_sustain(&mut self, ev: &SustainEvent) {
        if let Some(s) = &self.synth {
            s.apply_sustain(ev);
        }
    }

    /// Track a live `NoteEvent` for scoring/wait-mode only, without sounding
    /// it — for when the MIDI thread has already echoed it to the synth.
    pub fn track_held(&mut self, ev: NoteEvent) {
        self.held.apply(&ev);
        self.collect_strike(ev);
    }

    /// Record a note-on as a strike, stamped with the play clock rather than
    /// the device time: the chart lives in play-clock time, and device time
    /// keeps running while the clock is frozen or slowed. In a loop only *your
    /// turn* (and its count-in, for an early first note) collects; demo
    /// strikes just sound.
    fn collect_strike(&mut self, ev: NoteEvent) {
        let NoteEventKind::On { velocity } = ev.kind else {
            return;
        };
        if velocity.is_note_off() {
            return;
        }
        let stamped = NoteEvent::on(ev.note, velocity, self.now_us());
        match self.practice_loop.as_mut() {
            None => self.played.push(stamped),
            Some(rl) => {
                if matches!(
                    rl.lp.phase(),
                    LoopPhase::YourTurn
                        | LoopPhase::CountIn {
                            next: Pass::YourTurn
                        }
                ) {
                    rl.played.push(stamped);
                }
            }
        }
    }

    /// Whether span `i` is the player's to play (and so scored): every note
    /// with both hands practised, else only the practised hand's (in a loop,
    /// the hand as of the phase boundary). The other hand autoplays unscored.
    fn practised(&self, i: usize) -> bool {
        let practice = match &self.practice_loop {
            Some(rl) => rl.practice,
            None => self.practice,
        };
        practice.is_none_or(|h| self.hands[i] == h)
    }

    /// Judge every span whose good window has closed since the last call,
    /// then refresh the live figures — the desktop's `score_due`.
    fn score_due(&mut self) {
        let now = self.now_us();
        let good_us = self.cfg.good_us;
        let newly: Vec<usize> = match &self.practice_loop {
            None => (0..self.spans.len())
                .filter(|&i| {
                    !self.scored.contains(&i)
                        && now >= self.spans[i].start_us + good_us
                        && self.practised(i)
                })
                .collect(),
            // In a loop only *your turn* scores, and only the loop's notes.
            Some(rl) if rl.lp.phase() == LoopPhase::YourTurn => (0..self.spans.len())
                .filter(|&i| {
                    !rl.scored.contains(&i)
                        && rl.contains(&self.spans[i])
                        && now >= self.spans[i].start_us + good_us
                        && self.practised(i)
                })
                .collect(),
            Some(_) => return,
        };
        if newly.is_empty() {
            return;
        }
        match self.practice_loop.as_mut() {
            None => self.scored.extend(newly),
            Some(rl) => rl.scored.extend(newly),
        }
        self.recompute_live();
    }

    /// Recompute score, combo, hits and misses over the judged spans in time
    /// order with `core::score`, so the live numbers equal the final report.
    fn recompute_live(&mut self) {
        let (scored, played) = match &self.practice_loop {
            Some(rl) => (&rl.scored, &rl.played),
            None => (&self.scored, &self.played),
        };
        let report = score(&self.expected_for(scored.iter().copied()), played, self.cfg);
        (self.score, self.combo, self.best_combo) = (0, 0, 0);
        (self.live_hits, self.live_misses) = (0, 0);
        for j in &report.judgments {
            match j {
                NoteJudgment::Hit { timing, .. } => {
                    self.score += award(matches!(timing, Timing::Perfect), self.combo);
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

    /// The expected notes of the spans `indices`, in time order.
    fn expected_for(&self, indices: impl Iterator<Item = usize>) -> Vec<ExpectedNote> {
        let mut expected: Vec<ExpectedNote> = indices
            .filter_map(|i| {
                let s = &self.spans[i];
                MidiNote::new(s.note).map(|n| ExpectedNote::new(n, s.start_us))
            })
            .collect();
        expected.sort_by_key(|e| e.time_us);
        expected
    }

    /// The whole take's summary: every practised span against every strike.
    pub fn finish(&self) -> PlaySummary {
        let expected = self.expected_for((0..self.spans.len()).filter(|&i| self.practised(i)));
        let summary = Summary::from_report(&score(&expected, &self.played, self.cfg));
        PlaySummary {
            total_expected: summary.total_expected,
            hits: summary.hits,
            misses: summary.misses,
            extras: summary.extras,
            perfect: summary.perfect,
            early: summary.early,
            late: summary.late,
            accuracy_bp: accuracy_bp(&summary),
            best_combo: self.best_combo,
            score: self.score,
        }
    }

    /// The summary frozen at the song end, while the summary screen shows.
    pub fn summary(&self) -> Option<&PlaySummary> {
        self.summary.as_ref()
    }

    /// The live take for `play_status`. Reads only — never polls the gate.
    pub fn status(&self) -> PlayStatus {
        let awaiting = self.awaiting_notes().unwrap_or_default();
        PlayStatus {
            title: self.title.clone(),
            time_us: self.now_us(),
            duration_us: self.duration_us,
            paused: self.paused,
            frozen: self.paused || !awaiting.is_empty(),
            finished: self.is_finished(),
            wait_armed: self.wait.is_armed(),
            awaiting,
            held: self.held.iter().collect(),
            rate_permille: self.rate_permille,
            hear_song: self.hear_song,
            score: self.score,
            combo: self.combo,
            best_combo: self.best_combo,
            hits: self.live_hits,
            misses: self.live_misses,
            beats_per_bar: self.beats_per_bar,
            note_count: self.spans.len(),
            bar: self.current_bar(),
            practice_loop: self.loop_view(),
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

    /// The backing track's current level (the read path for tests).
    pub fn backing_gain(&self) -> Gain {
        self.backing_gain
    }

    /// The level the backing sink actually plays at: the fader, or silence
    /// while the take runs off-tempo (the recording cannot follow a changed
    /// speed without resampling).
    pub fn applied_backing_gain(&self) -> Gain {
        if self.is_off_tempo() {
            Gain::SILENT
        } else {
            self.backing_gain
        }
    }

    /// Set the backing track's level (M14-C). Applies to the live handle when
    /// the track is already playing, and is carried onto the sink the next
    /// [`tick_backing`](Self::tick_backing) creates.
    pub fn set_backing_gain(&mut self, gain: Gain) {
        self.backing_gain = gain;
        if let Some(h) = &self.backing_handle {
            h.set_gain(self.applied_backing_gain());
        }
    }

    /// Whether the "hear the song" audition is currently active (for the status
    /// line / tests).
    pub fn is_hear_song(&self) -> bool {
        self.hear_song
    }

    /// Check the playback clock and fire synth note_on / note_off commands for
    /// any song spans whose boundaries we've crossed since the last call.
    /// Call this once per event-loop iteration (not per render frame) to keep
    /// audio timing driven by the clock rather than the frame rate.
    pub fn tick_song_synth(&mut self) {
        // While the transport is stopped, hold the song back: its releases, so
        // the fade carries the sounding notes out instead of a damper cutting
        // them; its onsets, so it comes back in with the key you are waiting
        // on rather than before it.
        self.sync_parked_audio();
        if self.audio_parked {
            return;
        }
        let now = self.now_us();
        let (need_on, need_off) =
            pending_triggers(&self.spans, now, &self.song_on_fired, &self.song_off_fired);
        let need_on: Vec<usize> = need_on.into_iter().filter(|&i| self.autoplays(i)).collect();
        // A note already sounding is always released — even when a hand
        // change stopped it autoplaying mid-note.
        let need_off: Vec<usize> = need_off
            .into_iter()
            .filter(|&i| self.autoplays(i) || self.song_on_fired.contains(&i))
            .collect();
        let velocity = Velocity::new(HEAR_VELOCITY).unwrap();
        for i in need_on {
            if let Some(note) = MidiNote::new(self.spans[i].note) {
                if let Some(s) = &self.song_synth {
                    s.note_on(note, velocity);
                }
            }
            self.song_on_fired.insert(i);
        }
        for i in need_off {
            if let Some(note) = MidiNote::new(self.spans[i].note) {
                if let Some(s) = &self.song_synth {
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
        // A running loop keeps jumping back; it never ends the take.
        self.practice_loop.is_none() && self.now_us() > self.duration_us + self.finished_pause_us
    }

    // ── seeking + practice loop (M17-B) ──────────────────────────────────

    /// Whether the app sounds span `i` right now. Outside a loop: every note
    /// with "hear the song" on, and the *other* hand's whenever one hand is
    /// practised. In a loop (hand as of the phase boundary): never in a
    /// count-in; the demo plays the practised hand's loop notes (whatever
    /// "hear the song" says); *your turn* plays the other hand's, plus the
    /// practised hand's with "hear the song" on.
    fn autoplays(&self, i: usize) -> bool {
        let span = &self.spans[i];
        let hand = self.hands[i];
        match &self.practice_loop {
            None => self.hear_song || self.practice.is_some_and(|h| hand != h),
            Some(rl) => match rl.lp.phase() {
                LoopPhase::CountIn { .. } => false,
                LoopPhase::Demo => rl.contains(span) && rl.practice.is_none_or(|h| hand == h),
                LoopPhase::YourTurn => {
                    rl.contains(span) && (self.hear_song || rl.practice.is_some_and(|h| hand != h))
                }
            },
        }
    }

    /// The practised hand (`None` = both).
    pub fn practice(&self) -> Option<Hand> {
        self.practice
    }

    /// Practise one hand (`None` = both). Outside a loop the wait gate is
    /// rebuilt at the playhead from that hand's notes; a running loop picks
    /// the change up at its next phase boundary.
    pub fn set_practice(&mut self, practice: Option<Hand>) {
        self.practice = practice;
        if self.practice_loop.is_none() {
            self.reset_full_gate(self.now_us());
        }
    }

    /// `h`: both → right → left → both.
    pub fn cycle_practice(&mut self) {
        self.set_practice(match self.practice {
            None => Some(Hand::Right),
            Some(Hand::Right) => Some(Hand::Left),
            Some(Hand::Left) => None,
        });
    }

    /// Wait-gate steps for the notes of `practice` (every note when `None`)
    /// that `keep` accepts.
    fn steps_for(
        &self,
        practice: Option<Hand>,
        keep: impl Fn(&NoteSpan) -> bool,
    ) -> Vec<(MidiNote, u64)> {
        self.spans
            .iter()
            .zip(&self.hands)
            .filter(|&(s, &hand)| practice.is_none_or(|h| hand == h) && keep(s))
            .filter_map(|(s, _)| MidiNote::new(s.note).map(|n| (n, s.start_us)))
            .collect()
    }

    /// The bar under the playhead (0-based; the pre-roll is bar 0).
    pub fn current_bar(&self) -> u64 {
        self.bar_map().bar_at(self.now_us())
    }

    /// Rebuild the whole-song wait gate from the practised hand's notes,
    /// positioned at `us`, armed per the player's wait-mode setting.
    fn reset_full_gate(&mut self, us: u64) {
        self.wait = WaitGate::from_expected(&self.steps_for(self.practice, |_| true));
        self.wait.set_armed(self.wait_mode);
        self.wait.seek_to(us);
    }

    /// Jump the playhead to `us`: cut the song notes still sounding, seek the
    /// clock, reset the song triggers (pre-filling spans that end at or before
    /// `us`, so nothing bursts), re-seek the whole-song gate outside a loop (a
    /// running loop re-points its own at the phase it enters), and restart the
    /// backing — `tick_backing` re-arms it at the new position.
    pub fn seek_to(&mut self, us: u64) {
        self.cut_song_bus();
        self.clock.seek_us(us);
        self.song_on_fired.clear();
        self.song_off_fired.clear();
        for (i, span) in self.spans.iter().enumerate() {
            if span.end_us <= us {
                self.song_on_fired.insert(i);
                self.song_off_fired.insert(i);
            }
        }
        if self.practice_loop.is_none() {
            // Un-judge what lies ahead of the target so it is played afresh.
            let good_us = self.cfg.good_us;
            let spans = &self.spans;
            self.scored.retain(|&i| spans[i].start_us + good_us <= us);
            self.played.retain(|e| e.timestamp_us < us);
            self.recompute_live();
            self.reset_full_gate(us);
        }
        if let Some(h) = self.backing_handle.take() {
            h.stop();
        }
    }

    /// Release every song note still sounding, and the count-in click.
    fn cut_song_bus(&mut self) {
        for (i, span) in self.spans.iter().enumerate() {
            if self.song_on_fired.contains(&i) && !self.song_off_fired.contains(&i) {
                if let (Some(s), Some(note)) = (&self.song_synth, MidiNote::new(span.note)) {
                    s.note_off(note);
                }
            }
        }
        if self.click_off_at.take().is_some() {
            self.click_off();
        }
    }

    /// Pause the clock and the backing (the manual pause).
    fn pause_now(&mut self) {
        self.paused = true;
        if self.clock.is_running() {
            self.clock.pause();
        }
        self.sync_parked_audio();
    }

    /// `←` / `→`: pause, then jump to the start of the bar `delta` bars from
    /// the one under the playhead (clamped to the song). Outside a loop only;
    /// returns whether it moved.
    pub fn step_bar(&mut self, delta: i32) -> bool {
        if self.practice_loop.is_some() {
            return false;
        }
        self.pause_now();
        let map = self.bar_map();
        let last = map.bar_at(self.duration_us);
        let bar = (map.bar_at(self.now_us()) as i64 + delta as i64).clamp(0, last as i64) as u64;
        let target = map.bar_start(bar);
        self.seek_to(target);
        true
    }

    /// The marked bars as an ordered `(first, last)`: both marks, or the one
    /// set mark as a single bar.
    fn marked_range(&self) -> Option<(u64, u64)> {
        match self.loop_marks {
            (Some(a), Some(b)) => Some((a.min(b), a.max(b))),
            (Some(a), None) | (None, Some(a)) => Some((a, a)),
            (None, None) => None,
        }
    }

    /// `[`: loop start = the bar under the playhead. Restarts a running loop
    /// on the new range.
    pub fn mark_loop_start(&mut self) {
        self.loop_marks.0 = Some(self.current_bar());
        self.restart_if_looping();
    }

    /// `]`: loop end = the bar under the playhead (inclusive). Restarts a
    /// running loop on the new range.
    pub fn mark_loop_end(&mut self) {
        self.loop_marks.1 = Some(self.current_bar());
        self.restart_if_looping();
    }

    fn restart_if_looping(&mut self) {
        if self.practice_loop.is_some() {
            if let Some((a, b)) = self.marked_range() {
                self.start_loop(a, b);
            }
        }
    }

    /// `l`: stop a running loop, or start one over the marked bars — the bar
    /// under the playhead when nothing is marked.
    pub fn toggle_loop(&mut self) {
        if self.practice_loop.is_some() {
            self.clear_loop();
        } else {
            let (a, b) = self.marked_range().unwrap_or_else(|| {
                let bar = self.current_bar();
                (bar, bar)
            });
            self.set_loop(a, b);
        }
    }

    /// Mark bars `first..=last` (swapped if reversed) and start the practice
    /// loop over them from its count-in, unpausing the transport.
    pub fn set_loop(&mut self, first_bar: u64, last_bar: u64) {
        let (a, b) = (first_bar.min(last_bar), first_bar.max(last_bar));
        self.loop_marks = (Some(a), Some(b));
        self.start_loop(a, b);
    }

    fn start_loop(&mut self, first_bar: u64, last_bar: u64) {
        let map = self.bar_map();
        let (start_us, end_us) = map.bar_range_us(first_bar, last_bar);
        let count_in_us = map.bar_len(first_bar);
        let lp = PracticeLoop::new(start_us, end_us, count_in_us, self.beats_per_bar);
        let entry_us = lp.entry_us();
        self.practice_loop = Some(RunningLoop {
            lp,
            first_bar,
            last_bar,
            practice: self.practice,
            played: Vec::new(),
            scored: HashSet::new(),
        });
        self.last_pass = None;
        self.seek_to(entry_us);
        self.enter_phase(LoopPhase::CountIn { next: Pass::Demo });
        self.recompute_live();
        self.paused = false;
        if !self.clock.is_running() {
            self.clock.resume();
        }
    }

    /// Stop the running loop: leave it paused at the loop start with normal
    /// play restored (whole-song gate, triggers pre-filled, whole-take scoring
    /// resumed from there). With no loop running, clear the marks instead.
    pub fn clear_loop(&mut self) {
        match self.practice_loop.take() {
            Some(rl) => {
                self.pause_now();
                self.seek_to(rl.lp.start_us());
            }
            None => {
                self.loop_marks = (None, None);
                self.last_pass = None;
            }
        }
    }

    /// Apply a loop phase: snapshot the practised hand, and arm the loop's
    /// own wait gate for *your turn* only (built from the practised hand's
    /// loop notes, so a step at `end_us` is never in it). Count-in and demo
    /// run ungated.
    fn enter_phase(&mut self, phase: LoopPhase) {
        let practice = self.practice;
        let Some(rl) = self.practice_loop.as_mut() else {
            return;
        };
        rl.practice = practice;
        if phase == LoopPhase::YourTurn {
            let rl = self.practice_loop.as_ref().expect("loop running");
            let steps = self.steps_for(practice, |s| rl.contains(s));
            let start_us = rl.lp.start_us();
            self.wait = WaitGate::from_expected(&steps);
            self.wait.set_armed(self.wait_mode);
            self.wait.seek_to(start_us);
        } else {
            self.wait.set_armed(false);
        }
    }

    /// Step the loop's phase machine to the clock and act on it.
    fn tick_loop(&mut self) {
        let now = self.now_us();
        let Some(rl) = self.practice_loop.as_mut() else {
            return;
        };
        match rl.lp.tick(now) {
            LoopStep::Stay => {}
            LoopStep::Enter(phase) => self.enter_phase(phase),
            LoopStep::JumpTo {
                us,
                phase,
                pass_done,
            } => {
                if pass_done {
                    self.finish_pass();
                }
                // Each pass scores on its own, from zero.
                if let Some(rl) = self.practice_loop.as_mut() {
                    rl.played.clear();
                    rl.scored.clear();
                }
                self.recompute_live();
                self.seek_to(us);
                self.enter_phase(phase);
            }
        }
    }

    /// Score the *your turn* pass that just ended into `last_pass`, including
    /// loop notes whose window was still open at the wrap.
    fn finish_pass(&mut self) {
        let Some(rl) = self.practice_loop.as_ref() else {
            return;
        };
        let targets =
            (0..self.spans.len()).filter(|&i| rl.contains(&self.spans[i]) && self.practised(i));
        let expected = self.expected_for(targets);
        let summary = Summary::from_report(&score(&expected, &rl.played, self.cfg));
        self.last_pass = Some(PassSummary {
            pass: rl.lp.pass().saturating_sub(1),
            hits: summary.hits,
            misses: summary.misses,
            accuracy_bp: accuracy_bp(&summary),
        });
    }

    /// The loop's next phase boundary: the loop start during a count-in, the
    /// loop end during a pass.
    fn loop_boundary(&self) -> Option<u64> {
        let lp = &self.practice_loop.as_ref()?.lp;
        Some(match lp.phase() {
            LoopPhase::CountIn { .. } => lp.start_us(),
            LoopPhase::Demo | LoopPhase::YourTurn => lp.end_us(),
        })
    }

    /// Sound the count-in clicks for beats in `[prev_us, now_us)`, and release
    /// a click that has rung its length.
    fn loop_clicks(&mut self, prev_us: u64, now_us: u64) {
        if self.click_off_at.is_some_and(|t| now_us >= t) {
            self.click_off_at = None;
            self.click_off();
        }
        let Some(accent) = self
            .practice_loop
            .as_ref()
            .and_then(|rl| rl.lp.click_due(prev_us, now_us))
        else {
            return;
        };
        let vel = if accent {
            CLICK_VEL_ACCENT
        } else {
            CLICK_VEL_NORMAL
        };
        if let (Some(s), Some(note), Some(vel)) = (
            &self.song_synth,
            MidiNote::new(CLICK_NOTE),
            Velocity::new(vel),
        ) {
            // Re-strike cleanly if the previous click is still ringing.
            s.note_off(note);
            s.note_on(note, vel);
        }
        self.click_off_at = Some(now_us + CLICK_DUR_US);
    }

    fn click_off(&self) {
        if let (Some(s), Some(note)) = (&self.song_synth, MidiNote::new(CLICK_NOTE)) {
            s.note_off(note);
        }
    }

    /// The practice loop's view, or `None` when nothing is marked.
    pub fn loop_view(&self) -> Option<LoopView> {
        if let Some(rl) = &self.practice_loop {
            return Some(LoopView {
                first_bar: rl.first_bar,
                last_bar: rl.last_bar,
                start_us: rl.lp.start_us(),
                end_us: rl.lp.end_us(),
                running: true,
                phase: Some(rl.lp.phase()),
                pass: rl.lp.pass(),
                last_pass: self.last_pass,
            });
        }
        let (first_bar, last_bar) = self.marked_range()?;
        let (start_us, end_us) = self.bar_map().bar_range_us(first_bar, last_bar);
        Some(LoopView {
            first_bar,
            last_bar,
            start_us,
            end_us,
            running: false,
            phase: None,
            pass: 0,
            last_pass: self.last_pass,
        })
    }

    /// Notes the song wants held at the current instant (target set).
    /// The notes sounding at `now`, each with the colour its highway note has
    /// right now, so the key it lands on lights in the same colour.
    fn targets_now(&self, now: u64) -> Vec<(u8, Color)> {
        self.spans
            .iter()
            .zip(&self.hands)
            .filter(|(s, _)| s.start_us <= now && now < s.end_us)
            .map(|(s, &hand)| {
                (
                    s.note,
                    note_style(self.color_mode, s.note, hand, true).color,
                )
            })
            .collect()
    }

    /// The legend's "target" swatch: the right hand's sounding colour in
    /// hands mode, the accent otherwise (spectrum has no single colour).
    fn legend_target_color(&self) -> Color {
        note_style(self.color_mode, 60, Hand::Right, true).color
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
            let target = target_set.iter().find(|(n, _)| *n == note).map(|t| t.1);
            let is_held = held.is_held(note);
            match (target, is_held) {
                (Some(_), true) => Some(MATCH_COLOR), // hitting the right note
                (Some(c), false) => Some(c),          // song wants this now
                (None, true) => Some(HELD_COLOR),     // you're playing this
                (None, false) => None,
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
        if let Some(summary) = &self.summary {
            draw_summary(f, chunks[1], summary);
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
        let (hand_label, hand_color) = match self.practice {
            None => ("both", Color::DarkGray),
            Some(Hand::Right) => ("right", Color::Green),
            Some(Hand::Left) => ("left", Color::Green),
        };
        let pause_color = if self.paused {
            Color::Green
        } else {
            Color::DarkGray
        };
        // While paused, the transport badge reads PAUSED (yellow) instead of the
        // running PLAY badge, so the frozen state is unmistakable.
        let (badge_text, badge_bg) = if self.paused {
            (" PAUSED ", Color::Yellow)
        } else {
            (" PLAY ", TARGET_COLOR)
        };
        let mut spans = vec![
            Span::styled(badge_text, Style::default().fg(Color::Black).bg(badge_bg)),
            Span::raw(format!("  {:.1}s / {:.1}s  ", secs, total)),
            Span::styled(
                format!(
                    "score {}  combo {}  ",
                    group_thousands(self.score),
                    self.combo
                ),
                Style::default().fg(Color::Cyan),
            ),
            Span::styled(
                speed_badge(self.rate_permille),
                Style::default().fg(Color::Yellow),
            ),
            Span::raw("[r] restart  [Tab] menu  "),
            Span::styled("[Space] pause  ", Style::default().fg(pause_color)),
            Span::styled("[m] music  ", Style::default().fg(music_color)),
            Span::styled("[w] wait  ", Style::default().fg(wait_color)),
            Span::styled(
                format!("[h] {hand_label}  "),
                Style::default().fg(hand_color),
            ),
            Span::raw("[-/=] speed  "),
            Span::raw(format!("[c] {}  ", self.color_mode.label())),
            Span::raw(format!("[v] {}  ", self.scroll_mode.label())),
        ];
        // The loop badge sits by the clock, where a narrow terminal still
        // shows it; with nothing marked, a key hint joins the others instead.
        match self.loop_view() {
            Some(view) => {
                let style = if view.running {
                    Style::default().fg(Color::Black).bg(LOOP_BADGE.into())
                } else {
                    Style::default().fg(LOOP_BADGE.into())
                };
                spans.insert(2, Span::styled(loop_badge(&view), style));
                spans.insert(3, Span::raw("  "));
            }
            None => spans.push(Span::raw("[←→] bar  [ ] mark  [l] loop  ")),
        }
        if self.backing_playing() {
            // How often the backing had to be pulled back in step: a count
            // that keeps climbing points at audio underruns on this machine.
            let label = match self.backing_resyncs {
                0 => "♪ backing  ".to_string(),
                n => format!("♪ backing (resynced {n}×)  "),
            };
            spans.push(Span::styled(label, Style::default().fg(Color::Green)));
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
            Span::styled("● target ", Style::default().fg(self.legend_target_color())),
            Span::styled("● you ", Style::default().fg(HELD_COLOR)),
            Span::styled("● match", Style::default().fg(MATCH_COLOR)),
        ]);
        f.render_widget(Paragraph::new(Line::from(spans)), area);
    }

    /// Draw the highway in layers, back to front: the background with a faint
    /// tint on alternate white-key lanes; a lane guide under each lane's next
    /// note (a column of its colour, strengthening toward the keys, marking
    /// where it lands); the bar and beat lines; then the notes themselves.
    fn draw_highway(&self, f: &mut Frame, area: Rect, scale: Scale, x0: u16, now: u64) {
        if area.height == 0 {
            return;
        }
        let view = self.view_us(now);
        let w = scale.white_width();
        let board = Rect::new(x0, area.y, scale.total_width(), area.height).intersection(area);
        // A note's lane: its left column and width, matching the keyboard.
        let lane = |note: u8| -> Option<(u16, u16)> {
            if let Some(wi) = white_index(note) {
                Some((x0 + wi as u16 * w, w))
            } else if is_black_key(note) {
                black_key_col(note, scale).map(|c| (x0 + c, 1))
            } else {
                None
            }
        };
        let buf = f.buffer_mut();

        // 1. Background + lane tint.
        for y in area.top()..area.bottom() {
            for x in area.left()..area.right() {
                buf[(x, y)].set_bg(BACKGROUND.into());
            }
        }
        for note in 21u8..=108 {
            if white_index(note).is_none_or(|wi| wi % 2 != 0) {
                continue;
            }
            let Some((col, cw)) = lane(note) else {
                continue;
            };
            for y in area.top()..area.bottom() {
                for x in col..col + cw {
                    if area.contains((x, y).into()) {
                        buf[(x, y)].set_bg(LANE_TINT.into());
                    }
                }
            }
        }

        // 1b. The marked loop range: a faint band across the keyboard's width.
        if let Some(lv) = self.loop_view() {
            let band = NoteSpan {
                note: 0,
                start_us: lv.start_us,
                end_us: lv.end_us,
            };
            if let Some((top8, bottom8)) = span_extent8(&band, view, LEAD_US, area.height) {
                for (row, _) in cells_of_extent(top8, bottom8) {
                    let y = area.y + row;
                    for x in board.left()..board.right() {
                        if area.contains((x, y).into()) {
                            let under = bg_rgb(&buf[(x, y)]);
                            buf[(x, y)].set_bg(under.mix(LOOP_BADGE, LOOP_BAND).into());
                        }
                    }
                }
            }
        }

        // 2. Lane guides: black-key lanes last so they sit over the white ones,
        //    as on the keyboard. A guide runs from the first full row under
        //    its note's onset down to the keys.
        let mut guides = next_per_lane(&self.spans, view, LEAD_US);
        guides.sort_by_key(|&i| is_black_key(self.spans[i].note));
        for i in guides {
            let span = &self.spans[i];
            let (Some((_, onset8)), Some((col, cw))) = (
                span_extent8(span, view, LEAD_US, area.height),
                lane(span.note),
            ) else {
                continue;
            };
            let color = note_style(self.color_mode, span.note, self.hands[i], false).rgb;
            let (from, to) = ((onset8 + SUB - 1).div_euclid(SUB) as u16, area.height);
            for row in from..to {
                // Faint under the note, strongest at the keys.
                let t = (row - from + 1) as f32 / (to - from).max(1) as f32;
                let strength = GUIDE_AT_NOTE + (GUIDE_AT_KEYS - GUIDE_AT_NOTE) * t;
                let y = area.y + row;
                for x in col..col + cw {
                    if area.contains((x, y).into()) {
                        let under = bg_rgb(&buf[(x, y)]);
                        buf[(x, y)].set_bg(under.mix(color, strength).into());
                    }
                }
            }
        }

        // 3. Bar and beat lines across the keyboard's width, each at its
        //    height within its row.
        for g in grid_rows(
            &self.bar_map(),
            self.beats_per_bar,
            view,
            LEAD_US,
            area.height,
        ) {
            let (row, glyph) = line_cell(g.y8);
            let fg = if g.bar { BAR_LINE } else { BEAT_LINE };
            let y = area.y + row as u16;
            for x in board.left()..board.right() {
                buf[(x, y)].set_symbol(glyph).set_fg(fg.into());
            }
        }

        // 4. Notes, their edges to the eighth of a row: a partial top or
        //    bottom cell is a lower-block glyph (for a top edge, drawn inverted:
        //    the cell's background as the glyph, the note as the cell). The
        //    onset (bottom) edge is exact, so a note on a downbeat meets its
        //    bar line. Whether a note is *sounding* follows the real clock.
        for (span, &hand) in self.spans.iter().zip(&self.hands) {
            let (Some((top8, bottom8)), Some((col, cw))) = (
                span_extent8(span, view, LEAD_US, area.height),
                lane(span.note),
            ) else {
                continue;
            };
            let active = span.start_us <= now && now < span.end_us;
            let style = note_style(self.color_mode, span.note, hand, active);
            // While a loop runs, notes outside it fade back; so does the
            // other hand's while one hand is practised.
            let outside_loop = self
                .practice_loop
                .as_ref()
                .is_some_and(|rl| !rl.contains(span));
            let other_hand = self.practice.is_some_and(|h| hand != h);
            let color = if outside_loop || other_hand {
                style.rgb.mix(BACKGROUND, OUTSIDE_LOOP_FADE).into()
            } else {
                style.color
            };
            for (row, fill) in cells_of_extent(top8, bottom8) {
                let y = area.y + row;
                for x in col..col + cw {
                    if !area.contains((x, y).into()) {
                        continue;
                    }
                    let cell = &mut buf[(x, y)];
                    match fill {
                        CellFill::Full => {
                            cell.set_symbol(lower_block(8)).set_fg(color);
                        }
                        CellFill::Bottom(n) => {
                            cell.set_symbol(lower_block(n)).set_fg(color);
                        }
                        CellFill::Top(n) => {
                            let under = bg_rgb(cell);
                            cell.set_symbol(lower_block(8 - n))
                                .set_fg(under.into())
                                .set_bg(color);
                        }
                    }
                }
            }
        }
    }

    /// The song time the highway is drawn at: the clock itself when scrolling
    /// smoothly, or snapped back to the latest 16th in rhythm-step mode.
    fn view_us(&self, now: u64) -> u64 {
        match self.scroll_mode {
            ScrollMode::Smooth => now,
            ScrollMode::Sixteenths => step_to_sixteenth(&self.bar_map(), self.beats_per_bar, now),
        }
    }
}

/// A cell's background as RGB (the highway paints every cell's background, so
/// anything else is the plain highway background).
/// The end-of-song summary, boxed over the middle of the highway.
fn draw_summary(f: &mut Frame, area: Rect, summary: &PlaySummary) {
    let lines = summary_lines(summary);
    let width = lines.iter().map(|l| l.chars().count()).max().unwrap_or(0) as u16 + 6;
    let height = lines.len() as u16 + 2;
    let rect = Rect {
        x: area.x + area.width.saturating_sub(width) / 2,
        y: area.y + area.height.saturating_sub(height) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" song finished ");
    let text: Vec<Line> = lines
        .into_iter()
        .map(|l| Line::from(format!("  {l}")))
        .collect();
    f.render_widget(ratatui::widgets::Clear, rect);
    f.render_widget(Paragraph::new(text).block(block), rect);
}

fn bg_rgb(cell: &ratatui::buffer::Cell) -> Rgb {
    match cell.bg {
        Color::Rgb(r, g, b) => Rgb(r, g, b),
        _ => BACKGROUND,
    }
}

/// How the highway scrolls. Cycled with `v` on the play screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScrollMode {
    /// Continuously, to the eighth of a row.
    #[default]
    Smooth,
    /// In steps of one 16th note, on the rhythm of the bar grid.
    Sixteenths,
}

impl ScrollMode {
    /// The other mode.
    pub const fn next(self) -> Self {
        match self {
            ScrollMode::Smooth => ScrollMode::Sixteenths,
            ScrollMode::Sixteenths => ScrollMode::Smooth,
        }
    }

    /// Short label for the status line.
    pub const fn label(self) -> &'static str {
        match self {
            ScrollMode::Smooth => "smooth",
            ScrollMode::Sixteenths => "16th steps",
        }
    }
}

/// The loop's colour: its status badge and (faintly) its highway band.
const LOOP_BADGE: Rgb = Rgb(0xff, 0xd1, 0x66);
/// How strongly the loop band tints the highway.
const LOOP_BAND: f32 = 0.08;
/// How far notes outside a running loop fade toward the background.
const OUTSIDE_LOOP_FADE: f32 = 0.65;

/// The practice-speed badge by the clock — `0.75×  ` — or nothing at 1×.
pub fn speed_badge(rate_permille: u16) -> String {
    if rate_permille == PLAY_RATE_UNITY {
        String::new()
    } else {
        let x = format!("{:.3}", rate_permille as f64 / 1000.0);
        format!("{}×  ", x.trim_end_matches('0').trim_end_matches('.'))
    }
}

/// The status-line loop badge: `Loop 5–8` when marked, plus the phase and pass
/// while running (bars shown 1-based).
pub fn loop_badge(view: &LoopView) -> String {
    let bars = if view.first_bar == view.last_bar {
        format!("Loop {}", view.first_bar + 1)
    } else {
        format!("Loop {}–{}", view.first_bar + 1, view.last_bar + 1)
    };
    match view.phase {
        None => bars,
        Some(phase) => {
            let label = match phase {
                LoopPhase::CountIn { .. } => "COUNT-IN",
                LoopPhase::Demo => "DEMO",
                LoopPhase::YourTurn => "YOUR TURN",
            };
            match view.last_pass {
                Some(last) => format!(
                    " {bars} · {label} · pass {} · last {}% ",
                    view.pass,
                    percent(last.accuracy_bp)
                ),
                None => format!(" {bars} · {label} · pass {} ", view.pass),
            }
        }
    }
}

/// Basis points as a whole percentage, rounded.
fn percent(bp: u32) -> u32 {
    (bp + 50) / 100
}

/// `12340` → `12 340`: digits grouped in threes for the status line.
pub fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The summary screen's lines.
pub fn summary_lines(s: &PlaySummary) -> Vec<String> {
    vec![
        format!("Accuracy  {}%", percent(s.accuracy_bp)),
        format!("Score     {}", group_thousands(s.score)),
        format!("Best combo {}", s.best_combo),
        String::new(),
        format!(
            "Hits {}   Misses {}   Extras {}",
            s.hits, s.misses, s.extras
        ),
        format!(
            "Perfect {}   Early {}   Late {}",
            s.perfect, s.early, s.late
        ),
        String::new(),
        "[r] play again   [Tab] menu".to_string(),
    ]
}

/// Background of the tinted (alternate white-key) lanes.
const LANE_TINT: Rgb = Rgb(0x17, 0x18, 0x20);
/// Bar (downbeat) line colour — clearly visible, still behind the notes.
const BAR_LINE: Rgb = Rgb(0x55, 0x58, 0x68);
/// Beat line colour — a quieter version of the bar line.
const BEAT_LINE: Rgb = Rgb(0x2c, 0x2e, 0x3a);
/// Lane-guide strength (mix toward the note colour) just under the note…
const GUIDE_AT_NOTE: f32 = 0.06;
/// …and at the keyboard, where the eye should land.
const GUIDE_AT_KEYS: f32 = 0.22;

/// Visual style for one highway note block: the colour to paint it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct NoteStyle {
    pub color: Color,
    /// `color` as RGB, for blending (lane guides).
    pub rgb: Rgb,
}

/// Choose the [`NoteStyle`] for a single highway note block.
///
/// The colour comes from the [`ColorMode`] (by hand, pitch class, or one
/// accent — see [`crate::palette`]). Two cues layer on top, so the highway
/// stays readable in every mode:
///
/// 1. **active vs upcoming** — a note sounding at the current clock position is
///    lifted brighter than one still falling.
/// 2. **white key vs black key** — a black-key (accidental) note is *dimmer*,
///    on top of being drawn one column wide.
///
/// Notes are solid blocks: their edges are drawn to the eighth of a row with
/// the lower-block glyphs (smooth scrolling), which a shaded fill (the former
/// `▓`/`▒` accidental cue) could not match.
pub fn note_style(mode: ColorMode, note: u8, hand: Hand, active: bool) -> NoteStyle {
    let rgb = note_color(mode, note, hand, active, is_black_key(note));
    NoteStyle {
        color: rgb.into(),
        rgb,
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

    // ── chart audition default (issue #152) ─────────────────────────────────

    #[test]
    fn hear_song_defaults_off_but_builder_enables_it() {
        // Play-along default: the song does not sound over the player.
        let play = one_note_screen();
        assert!(!play.is_hear_song());
        // Imports opt in so the chart is audible on load.
        let play = one_note_screen().with_hear_song(true);
        assert!(play.is_hear_song());
    }

    #[test]
    fn pending_triggers_drive_audition_for_imported_chart() {
        // An auditioned chart fires note_on at the (shifted) note start. The one
        // note is at t=0, so after the whole-song shift it starts at SHIFT.
        let play = one_note_screen().with_hear_song(true);
        let shift = PRE_ROLL_US + LEAD_US;
        let (on, _off) = pending_triggers(&play.spans, shift, &HashSet::new(), &HashSet::new());
        assert_eq!(
            on,
            vec![0],
            "the chart note is due to sound at the shift point"
        );
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

    // ── freeze fade ──────────────────────────────────────────────────────

    /// Legato: C over 0–1 s, then D over 1–2 s, so C ends exactly where D (a
    /// wait step) begins. Shifted: C at `SHIFT`, D at `SHIFT + 1 s`.
    fn legato_screen() -> PlayScreen {
        let c = MidiNote::new(60).unwrap();
        let d = MidiNote::new(62).unwrap();
        let v = Velocity::new(80).unwrap();
        let events = vec![
            NoteEvent::on(c, v, 0),
            NoteEvent::off(c, 1_000_000),
            NoteEvent::on(d, v, 1_000_000),
            NoteEvent::off(d, 2_000_000),
        ];
        PlayScreen::from_smf_bytes("legato".into(), &events_to_smf_bytes(&events), None)
            .unwrap()
            .with_hear_song(true)
    }

    #[test]
    fn a_long_tick_lands_on_the_wait_step_and_parks_there() {
        let mut play = two_note_screen();
        play.set_wait_mode(true);
        // One long tick from the top would overshoot C's onset; it stops on it.
        play.advance(SHIFT + 400_000);
        assert_eq!(play.now_us(), SHIFT);
        assert!(play.parked(), "parked on the very tick it lands");
    }

    #[test]
    fn a_wait_freeze_fades_the_song_instead_of_releasing_it() {
        let mut play = legato_screen();
        play.set_wait_mode(true);
        note_on(&mut play, 60);
        play.advance(SHIFT);
        play.tick_song_synth();
        let c = 0;
        assert!(play.song_on_fired.contains(&c), "C sounds");
        // One long tick reaches D's onset, where C ends: the freeze is seen
        // on that tick, so C's release is held for the fade to carry out.
        note_off(&mut play, 60);
        play.advance(2_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
        play.tick_song_synth();
        assert!(play.audio_parked);
        assert!(!play.song_off_fired.contains(&c), "C fades, not cut");
        // Play D: the song comes back with it.
        note_on(&mut play, 62);
        play.advance(10_000);
        play.tick_song_synth();
        assert!(!play.audio_parked);
        assert!(play.song_off_fired.contains(&c));
        assert!(play.song_on_fired.contains(&1), "D sounds on the thaw");
    }

    #[test]
    fn pausing_fades_at_once_and_resuming_brings_it_back() {
        let mut play = legato_screen();
        play.advance(SHIFT + 100_000);
        play.tick_song_synth();
        play.toggle_pause();
        assert!(play.audio_parked, "on the keystroke, not a tick later");
        // Nothing fires while paused, even past a note boundary on resume's
        // side of things.
        let fired = play.song_on_fired.len();
        play.advance(2_000_000);
        play.tick_song_synth();
        assert_eq!(play.song_on_fired.len(), fired);
        play.toggle_pause();
        assert!(!play.audio_parked);
    }

    #[test]
    fn restart_clears_the_parked_audio() {
        let mut play = legato_screen();
        play.toggle_pause();
        assert!(play.audio_parked);
        play.restart();
        assert!(!play.audio_parked);
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

    // ── manual pause/resume (M12-A, issue #231) ──────────────────────────────

    #[test]
    fn toggle_pause_freezes_then_resumes_from_the_same_position() {
        let mut play = two_note_screen();
        // Advance partway through the lead-in, then pause.
        play.advance(1_000_000);
        assert_eq!(play.now_us(), 1_000_000);
        play.toggle_pause();
        assert!(play.is_paused());
        // While paused, wall-time deltas do NOT move the playhead.
        play.advance(5_000_000);
        play.advance(5_000_000);
        assert_eq!(play.now_us(), 1_000_000, "clock frozen while paused");
        // Resuming continues from exactly where it froze.
        play.toggle_pause();
        assert!(!play.is_paused());
        play.advance(500_000);
        assert_eq!(play.now_us(), 1_500_000, "resumes from the frozen position");
    }

    #[test]
    fn pause_is_independent_of_wait_mode() {
        // Wait-mode disarmed: pause still freezes the free-running highway.
        let mut play = two_note_screen();
        assert!(!play.is_wait_mode());
        play.toggle_pause();
        play.advance(SHIFT + 5_000_000);
        assert_eq!(play.now_us(), 0, "paused clock never advances");
        play.toggle_pause();
        play.advance(SHIFT);
        assert_eq!(play.now_us(), SHIFT);
    }

    #[test]
    fn resuming_pause_stays_frozen_when_wait_mode_awaits_a_step() {
        // Pause on top of an armed, unsatisfied wait step. Un-pausing must NOT
        // thaw past the step — wait-mode still holds it until the note is held.
        let mut play = two_note_screen();
        play.set_wait_mode(true);
        play.advance(SHIFT); // parked on the C step, nothing held
        play.toggle_pause();
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT);
        // Un-pause: still frozen because the C step is unsatisfied.
        play.toggle_pause();
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT, "wait-mode keeps the clock frozen");
        // Holding C releases both freezes and the clock advances.
        note_on(&mut play, 60);
        play.advance(1_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
    }

    #[test]
    fn restart_clears_the_pause() {
        let mut play = two_note_screen();
        play.toggle_pause();
        assert!(play.is_paused());
        play.restart();
        assert!(!play.is_paused(), "restart un-pauses");
        play.advance(1_000_000);
        assert_eq!(play.now_us(), 1_000_000, "clock runs again after restart");
    }

    // ── note colouring by hand ────────────────────────────────────────────────

    #[test]
    fn hands_default_to_the_middle_c_split() {
        // C4 (60) and D4 (62) are both at/above middle C: right hand.
        let play = two_note_screen();
        assert_eq!(play.hands(), &[Hand::Right, Hand::Right]);
    }

    #[test]
    fn with_hands_applies_the_split_and_per_note_overrides() {
        let play = two_note_screen();
        // Split above both notes: both left…
        let split_only = two_note_screen().with_hands(64, &[]);
        assert_eq!(split_only.hands(), &[Hand::Left, Hand::Left]);
        // …except D, pinned right by an override keyed at its file position
        // (the span's start before the lead-in shift).
        let d_file_start = play.spans[1].start_us - play.shift_us;
        let overrides = [HandOverride {
            pitch: 62,
            start_us: d_file_start,
            hand: Hand::Right,
        }];
        let pinned = two_note_screen().with_hands(64, &overrides);
        assert_eq!(pinned.hands(), &[Hand::Left, Hand::Right]);
    }

    #[test]
    fn sounding_targets_carry_their_hand_colour() {
        let play = two_note_screen().with_hands(61, &[]); // C left, D right
        let c_on = play.spans[0].start_us;
        let targets = play.targets_now(c_on);
        assert_eq!(
            targets,
            vec![(60, note_style(ColorMode::Hands, 60, Hand::Left, true).color)]
        );
    }

    // ── lanes + bar lines ─────────────────────────────────────────────────────

    #[test]
    fn with_grid_moves_the_tempo_map_into_play_time() {
        let play = one_note_screen();
        let shift = play.shift_us;
        let play = play.with_grid(None, &[0, 1_000_000, 2_500_000]);
        let map = play.bar_map();
        assert_eq!(map.bar_start(0), shift);
        assert_eq!(map.bar_start(2), 2_500_000 + shift);
    }

    #[test]
    fn without_a_grid_bars_are_120_bpm_from_the_first_note() {
        let play = one_note_screen().with_grid(None, &[]);
        let map = play.bar_map();
        // 120 BPM 4/4 = 2 s bars, bar 0 on the (shifted) first note.
        assert_eq!(map.bar_start(0), play.shift_us);
        assert_eq!(map.bar_len(0), 2_000_000);
    }

    /// Render `play` at `now` into a `width`×`height` test terminal.
    fn render(play: &mut PlayScreen, now: u64, width: u16, height: u16) -> ratatui::buffer::Buffer {
        play.advance(now - play.now_us());
        let mut term =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, height)).unwrap();
        term.draw(|f| play.draw(f, f.area())).unwrap();
        term.backend().buffer().clone()
    }

    /// Each row of `buf` as a string of its cell symbols.
    fn rows(buf: &ratatui::buffer::Buffer) -> Vec<String> {
        (0..buf.area.height)
            .map(|y| (0..buf.area.width).map(|x| buf[(x, y)].symbol()).collect())
            .collect()
    }

    /// The first highway row drawn as a line (a run of one line glyph).
    fn line_row(rows: &[String]) -> Option<usize> {
        rows.iter().position(|r| {
            // Inside the highway box: a border row starts with a corner.
            r.starts_with('│')
                && ["▔", "⎺", "─", "⎽", "▁"]
                    .iter()
                    .any(|g| r.contains(&g.repeat(6)))
        })
    }

    #[test]
    fn a_note_on_a_downbeat_meets_its_bar_line_and_casts_a_lane_guide() {
        // Bar 0 is on the first note; 1.5 s before it, both sit up the highway.
        let mut play = one_note_screen();
        let first = play.spans[0].start_us;
        for now in [first - 1_500_000, first - 1_234_567] {
            let buf = render(&mut play, now, 120, 30);
            let rows = rows(&buf);
            let bar_row = line_row(&rows).expect("a bar line is drawn") as u16;
            // The note's onset edge is in the bar line's row: that row holds a
            // cell painted in the note colour (as glyph or, at a top edge, as
            // the cell background).
            let note = note_style(ColorMode::Hands, 60, Hand::Right, false).color;
            let note_x = (0..buf.area.width)
                .find(|&x| buf[(x, bar_row)].fg == note || buf[(x, bar_row)].bg == note)
                .unwrap_or_else(|| {
                    panic!(
                        "note on its bar line (now = {now}): {}",
                        rows[bar_row as usize]
                    )
                });
            // Below it, its lane carries a guide that strengthens toward the keys.
            let highway_bottom = (bar_row..buf.area.height)
                .take_while(|&y| rows[y as usize].starts_with('│'))
                .last()
                .unwrap();
            let near = buf[(note_x, bar_row + 2)].bg;
            let keys = buf[(note_x, highway_bottom)].bg;
            assert_ne!(near, keys, "the guide strengthens toward the keys");
        }
    }

    #[test]
    fn smooth_mode_moves_within_a_row_sixteenth_mode_holds_until_the_next_step() {
        // 120 BPM 4/4 (no grid): a 16th is 125 ms; 1.5 s before bar 0 is a step.
        let first = one_note_screen().spans[0].start_us;
        let at = |mode: ScrollMode, now: u64| {
            let mut play = one_note_screen();
            play.set_scroll_mode(mode);
            // The highway and keyboard only: the status line shows the clock.
            rows(&render(&mut play, now, 120, 30)).split_off(1)
        };
        let a = at(ScrollMode::Smooth, first - 1_500_000);
        let b = at(ScrollMode::Smooth, first - 1_470_000); // 30 ms later, same 16th
        assert_ne!(a, b, "smooth: 30 ms (~2.8 eighths here) is visible");

        let a = at(ScrollMode::Sixteenths, first - 1_500_000);
        let b = at(ScrollMode::Sixteenths, first - 1_380_000); // same 16th step
        let c = at(ScrollMode::Sixteenths, first - 1_375_000); // the next step
        assert_eq!(a, b, "16th steps: holds within a step");
        assert_ne!(b, c, "16th steps: moves on the step");
    }

    // ── highway key-note distinction (M11-A, issue #229) ─────────────────────

    #[test]
    fn black_and_white_keys_differ_in_color() {
        // C (60, white) vs C# (61, black) at the same `active` value differ in
        // colour (black-key notes are also one column narrow). The former
        // shaded-glyph cue gave way to solid blocks for sub-row edges.
        for mode in [ColorMode::Hands, ColorMode::Spectrum, ColorMode::Accent] {
            for active in [true, false] {
                let white = note_style(mode, 60, Hand::Right, active);
                let black = note_style(mode, 61, Hand::Right, active);
                assert_ne!(white.color, black.color, "{mode:?} active={active}");
            }
        }
    }

    #[test]
    fn active_and_upcoming_differ_for_a_given_pitch() {
        // The pre-existing active/upcoming signal survives for both key kinds.
        for note in [60u8, 61] {
            let on = note_style(ColorMode::Hands, note, Hand::Right, true);
            let off = note_style(ColorMode::Hands, note, Hand::Right, false);
            assert_ne!(on.color, off.color, "note={note}: active vs upcoming");
        }
    }

    // ── practice loop (M17-B) ────────────────────────────────────────────

    /// A note every second (half a bar at the default 120 BPM, 4/4), pitches
    /// 60, 61, … for eight seconds. Bars are 2 s from `SHIFT`, so bar 1 is
    /// `SHIFT + 2 s .. SHIFT + 4 s` and holds the notes at +2 s and +3 s.
    fn loop_screen() -> PlayScreen {
        let v = Velocity::new(80).unwrap();
        let events: Vec<NoteEvent> = (0..8u64)
            .flat_map(|i| {
                let n = MidiNote::new(60 + i as u8).unwrap();
                [
                    NoteEvent::on(n, v, i * 1_000_000),
                    NoteEvent::off(n, i * 1_000_000 + 500_000),
                ]
            })
            .collect();
        PlayScreen::from_smf_bytes("loop".into(), &events_to_smf_bytes(&events), None).unwrap()
    }

    const BAR: u64 = 2_000_000;

    fn phase(play: &PlayScreen) -> Option<LoopPhase> {
        play.loop_view().and_then(|v| v.phase)
    }

    /// Index of the span starting at `us`.
    fn span_at(play: &PlayScreen, us: u64) -> usize {
        play.spans.iter().position(|s| s.start_us == us).unwrap()
    }

    #[test]
    fn set_loop_enters_the_count_in_a_bar_before_the_loop() {
        let mut play = loop_screen();
        play.set_loop(2, 1); // reversed: swapped
        let view = play.loop_view().unwrap();
        assert_eq!((view.first_bar, view.last_bar), (1, 2));
        assert_eq!((view.start_us, view.end_us), (SHIFT + BAR, SHIFT + 3 * BAR));
        assert!(view.running);
        assert_eq!(view.pass, 1);
        assert_eq!(play.now_us(), SHIFT, "one bar before the loop");
        assert_eq!(phase(&play), Some(LoopPhase::CountIn { next: Pass::Demo }));
        assert_eq!(loop_badge(&view), " Loop 2–3 · COUNT-IN · pass 1 ");
    }

    #[test]
    fn a_full_cycle_switches_phases_on_the_bar_lines() {
        let mut play = loop_screen();
        play.set_loop(1, 1);
        // A long tick lands exactly on the loop start, not past it.
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT + BAR);
        assert_eq!(phase(&play), Some(LoopPhase::Demo));
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT, "the pass end jumps to the count-in");
        assert_eq!(
            phase(&play),
            Some(LoopPhase::CountIn {
                next: Pass::YourTurn
            })
        );
        play.advance(BAR);
        assert_eq!(phase(&play), Some(LoopPhase::YourTurn));
        play.advance(BAR);
        assert_eq!(play.now_us(), SHIFT);
        assert_eq!(phase(&play), Some(LoopPhase::CountIn { next: Pass::Demo }));
        assert_eq!(play.loop_view().unwrap().pass, 2);
    }

    #[test]
    fn the_demo_plays_the_loop_whatever_hear_the_song_says() {
        let mut play = loop_screen();
        assert!(!play.is_hear_song());
        play.set_loop(1, 1);
        // Count-in: nothing sounds, not even the notes it scrolls past.
        play.advance(BAR / 2);
        play.tick_song_synth();
        assert!(play.song_on_fired.is_empty(), "count-in is silent");
        // Demo: the loop's notes sound as the clock reaches them.
        play.advance(BAR);
        play.tick_song_synth();
        let first = span_at(&play, SHIFT + BAR);
        assert!(play.song_on_fired.contains(&first));
        play.advance(BAR);
        play.tick_song_synth();
        assert!(!play.song_on_fired.contains(&span_at(&play, SHIFT)));
        // Your turn with "hear the song" off: silent.
        play.advance(BAR); // the count-in reaches your turn
        assert_eq!(phase(&play), Some(LoopPhase::YourTurn));
        play.advance(BAR / 2);
        play.tick_song_synth();
        assert!(play.song_on_fired.is_empty(), "your turn is yours");
        // The next demo sounds the loop again (triggers re-fire each pass).
        play.advance(BAR); // wrap to the count-in
        play.advance(BAR); // the demo starts
        assert_eq!(phase(&play), Some(LoopPhase::Demo));
        play.tick_song_synth();
        assert!(play.song_on_fired.contains(&first), "re-fires on pass 2");
    }

    #[test]
    fn wait_mode_gates_only_your_turn_and_never_the_loop_end() {
        let mut play = loop_screen();
        play.set_wait_mode(true);
        play.set_loop(1, 1);
        // Count-in and demo run freely past their notes.
        play.advance(BAR);
        play.advance(BAR / 2);
        assert_eq!(play.now_us(), SHIFT + BAR + BAR / 2, "demo not gated");
        play.advance(BAR);
        play.advance(BAR); // count-in → your turn
        assert_eq!(phase(&play), Some(LoopPhase::YourTurn));
        // Your turn freezes on its first note until it is played.
        play.advance(100_000);
        play.advance(100_000);
        assert_eq!(play.now_us(), SHIFT + BAR, "waits for the note");
        note_on(&mut play, 62);
        play.advance(100_000);
        assert!(play.now_us() > SHIFT + BAR);
        note_off(&mut play, 62);
        play.advance(BAR / 2 - 100_000); // up to the second note
        note_on(&mut play, 63);
        play.advance(100_000);
        note_off(&mut play, 63);
        // The note at exactly the loop end (65) is not the loop's to wait for.
        play.advance(BAR);
        assert_eq!(
            play.now_us(),
            SHIFT,
            "wrapped instead of freezing at the end"
        );
        assert_eq!(phase(&play), Some(LoopPhase::CountIn { next: Pass::Demo }));
        assert!(
            play.is_wait_mode(),
            "the setting survives the ungated phases"
        );
    }

    #[test]
    fn stopping_the_loop_pauses_at_its_start_with_normal_play_back() {
        let mut play = loop_screen();
        play.set_wait_mode(true);
        play.set_loop(1, 1);
        play.advance(BAR + BAR / 2);
        play.toggle_loop();
        assert!(play.is_paused());
        assert_eq!(play.now_us(), SHIFT + BAR);
        let view = play.loop_view().unwrap();
        assert!(!view.running, "the marks stay");
        assert_eq!(loop_badge(&view), "Loop 2");
        // The whole-song gate is back, armed, at the loop start.
        assert!(play.wait.is_armed());
        play.toggle_pause();
        play.advance(100_000);
        assert_eq!(
            play.now_us(),
            SHIFT + BAR,
            "waits for the note at the start"
        );
        // Clearing with no loop running drops the marks.
        play.clear_loop();
        assert!(play.loop_view().is_none());
    }

    #[test]
    fn step_bar_pauses_and_lands_on_bar_starts_outside_a_loop() {
        let mut play = loop_screen();
        play.advance(SHIFT + BAR + 300_000);
        assert!(play.step_bar(1));
        assert!(play.is_paused());
        assert_eq!(play.now_us(), SHIFT + 2 * BAR);
        assert!(play.step_bar(-5), "clamped at the first bar");
        assert_eq!(play.now_us(), SHIFT);
        // Past-ended spans are pre-filled, so nothing bursts on resume.
        assert!(play.step_bar(2));
        assert!(play.song_on_fired.contains(&span_at(&play, SHIFT)));
        play.set_loop(0, 0);
        assert!(!play.step_bar(1), "refused while looping");
    }

    #[test]
    fn marks_follow_the_playhead_and_restart_a_running_loop() {
        let mut play = loop_screen();
        play.advance(SHIFT + BAR + 1);
        play.mark_loop_start();
        play.advance(BAR);
        play.mark_loop_end();
        let view = play.loop_view().unwrap();
        assert_eq!((view.first_bar, view.last_bar, view.running), (1, 2, false));
        play.toggle_loop();
        play.advance(BAR); // into the demo
        play.mark_loop_end(); // playhead in bar 1: the loop becomes bar 1 only
        let view = play.loop_view().unwrap();
        assert_eq!((view.first_bar, view.last_bar), (1, 1));
        assert_eq!(phase(&play), Some(LoopPhase::CountIn { next: Pass::Demo }));
        assert_eq!(play.now_us(), SHIFT, "restarted from its count-in");
    }

    #[test]
    fn the_loop_badge_fits_an_80_column_status_line() {
        let mut play = loop_screen();
        let hint = rows(&render(&mut play, 0, 200, 12))[0].clone();
        assert!(hint.contains("[l] loop"), "{hint}");
        play.set_loop(1, 1);
        let now = play.now_us();
        let status = rows(&render(&mut play, now, 80, 12))[0].clone();
        assert!(status.contains("Loop 2 · COUNT-IN · pass 1"), "{status}");
    }

    #[test]
    fn a_loop_on_the_last_bar_never_finishes_the_song() {
        let mut play = loop_screen();
        let last = play.bar_map().bar_at(play.duration_us);
        play.set_loop(last, last);
        for _ in 0..50 {
            play.advance(BAR);
            assert!(!play.is_finished());
        }
        play.toggle_loop();
        play.toggle_pause();
        play.advance(10 * BAR);
        assert!(play.is_finished(), "normal play ends again");
    }

    // ── practice hand (M18-C) ────────────────────────────────────────────

    /// A two-hand chart, split at middle C: a note every second, even seconds
    /// left (48 + i), odd seconds right (72 + i) — so bar 1 holds left 50 at
    /// `SHIFT + 2 s` and right 75 at `SHIFT + 3 s`. The left-range note at
    /// 6 s (54) carries a per-note override to the right hand.
    fn two_hand_screen() -> PlayScreen {
        let v = Velocity::new(80).unwrap();
        let events: Vec<NoteEvent> = (0..8u64)
            .flat_map(|i| {
                let pitch = if i % 2 == 0 { 48 + i } else { 72 + i } as u8;
                let n = MidiNote::new(pitch).unwrap();
                [
                    NoteEvent::on(n, v, i * 1_000_000),
                    NoteEvent::off(n, i * 1_000_000 + 500_000),
                ]
            })
            .collect();
        let overrides = [HandOverride {
            pitch: 54,
            start_us: 6_000_000,
            hand: Hand::Right,
        }];
        PlayScreen::from_smf_bytes("hands".into(), &events_to_smf_bytes(&events), None)
            .unwrap()
            .with_hands(60, &overrides)
    }

    fn pitches(steps: &[(MidiNote, u64)]) -> Vec<u8> {
        steps.iter().map(|(n, _)| n.value()).collect()
    }

    #[test]
    fn only_the_practised_hand_is_scored() {
        let mut play = two_hand_screen();
        assert_eq!(play.finish().total_expected, 8, "both hands: every note");
        play.set_practice(Some(Hand::Right));
        // 73, 75, 77, 79 plus the 54 pinned to the right hand.
        assert_eq!(play.finish().total_expected, 5);
        play.set_practice(Some(Hand::Left));
        assert_eq!(play.finish().total_expected, 3);
    }

    #[test]
    fn right_hand_practice_waits_for_right_and_autoplays_left() {
        let mut play = two_hand_screen();
        assert!(!play.is_hear_song());
        play.set_wait_mode(true);
        play.set_practice(Some(Hand::Right));
        // The gate holds the right hand's notes, the override among them.
        assert_eq!(
            pitches(&play.steps_for(play.practice(), |_| true)),
            vec![73, 75, 77, 54, 79]
        );
        let overridden = span_at(&play, SHIFT + 6_000_000);
        assert!(!play.autoplays(overridden), "the override follows its hand");
        // The left note at SHIFT neither freezes the clock nor stays silent.
        play.advance(SHIFT);
        play.advance(500_000);
        assert_eq!(
            play.now_us(),
            SHIFT + 500_000,
            "left notes are not waited on"
        );
        play.tick_song_synth();
        assert!(play.song_on_fired.contains(&span_at(&play, SHIFT)));
        // The right note at SHIFT + 1 s freezes it until played.
        play.advance(5_000_000);
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT + 1_000_000);
        assert_eq!(play.awaiting_notes(), Some(vec![73]));
        play.tick_song_synth();
        let held_back = span_at(&play, SHIFT + 1_000_000);
        assert!(!play.song_on_fired.contains(&held_back));
        note_on(&mut play, 73);
        play.advance(100_000);
        assert!(play.now_us() > SHIFT + 1_000_000);
    }

    #[test]
    fn both_hands_autoplay_nothing_without_hear_the_song() {
        let mut play = two_hand_screen();
        assert_eq!(play.practice(), None);
        for _ in 0..20 {
            play.advance(1_000_000);
            play.tick_song_synth();
        }
        assert!(play.song_on_fired.is_empty());
    }

    #[test]
    fn changing_the_hand_mid_song_rebuilds_the_gate_at_the_playhead() {
        let mut play = two_hand_screen();
        play.set_wait_mode(true);
        play.set_practice(Some(Hand::Left));
        play.advance(SHIFT);
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT, "frozen on the left step");
        assert_eq!(play.awaiting_notes(), Some(vec![48]));
        play.set_practice(Some(Hand::Right));
        play.advance(500_000);
        assert_eq!(play.now_us(), SHIFT + 500_000, "un-frozen by the switch");
        play.advance(5_000_000);
        play.advance(5_000_000);
        assert_eq!(play.awaiting_notes(), Some(vec![73]), "the next right step");
    }

    #[test]
    fn a_loop_demos_the_practised_hand_and_accompanies_your_turn() {
        let mut play = two_hand_screen();
        play.set_wait_mode(true);
        play.set_practice(Some(Hand::Right));
        play.set_loop(1, 1);
        let (left, right) = (
            span_at(&play, SHIFT + BAR),
            span_at(&play, SHIFT + BAR + 1_000_000),
        );
        // Demo: the right note sounds, the left one doesn't.
        play.advance(BAR);
        assert_eq!(phase(&play), Some(LoopPhase::Demo));
        play.advance(1_000_000);
        play.tick_song_synth();
        assert!(play.song_on_fired.contains(&right));
        assert!(!play.song_on_fired.contains(&left));
        // Your turn: the left note autoplays, the gate waits for the right.
        play.advance(BAR); // wrap to the count-in
        play.advance(BAR); // → your turn
        assert_eq!(phase(&play), Some(LoopPhase::YourTurn));
        play.tick_song_synth();
        assert_eq!(play.song_on_fired, HashSet::from([left]));
        play.advance(5_000_000);
        play.advance(5_000_000);
        assert_eq!(play.now_us(), SHIFT + BAR + 1_000_000, "waits on the right");
        assert_eq!(play.awaiting_notes(), Some(vec![75]));
        play.tick_song_synth();
        assert!(!play.song_on_fired.contains(&right), "your turn is yours");
    }

    #[test]
    fn a_hand_change_mid_loop_applies_from_the_next_count_in() {
        let mut play = two_hand_screen();
        play.set_wait_mode(true);
        play.set_practice(Some(Hand::Right));
        play.set_loop(1, 1);
        let (left, right) = (
            span_at(&play, SHIFT + BAR),
            span_at(&play, SHIFT + BAR + 1_000_000),
        );
        play.advance(BAR);
        assert_eq!(phase(&play), Some(LoopPhase::Demo));
        play.set_practice(Some(Hand::Left));
        // The rest of this demo still plays the right hand.
        play.advance(1_000_000);
        play.tick_song_synth();
        assert!(play.song_on_fired.contains(&right));
        assert!(!play.song_on_fired.contains(&left));
        // From the count-in on, the left hand is practised.
        play.advance(BAR);
        play.advance(BAR);
        assert_eq!(phase(&play), Some(LoopPhase::YourTurn));
        play.advance(100_000);
        assert_eq!(play.now_us(), SHIFT + BAR, "waits on the left");
        assert_eq!(play.awaiting_notes(), Some(vec![50]));
        note_on(&mut play, 50);
        play.advance(1_000_000);
        play.tick_song_synth();
        assert!(play.song_on_fired.contains(&right), "right accompanies");
        assert!(!play.song_on_fired.contains(&left));
    }

    #[test]
    fn the_status_line_shows_the_practised_hand_and_restart_keeps_it() {
        let mut play = two_hand_screen();
        let status = rows(&render(&mut play, 0, 200, 12))[0].clone();
        assert!(status.contains("[h] both"), "{status}");
        play.cycle_practice();
        let now = play.now_us();
        let status = rows(&render(&mut play, now, 200, 12))[0].clone();
        assert!(status.contains("[h] right"), "{status}");
        play.restart();
        assert_eq!(play.practice(), Some(Hand::Right));
    }

    // ── practice speed (M18-B) ───────────────────────────────────────────

    #[test]
    fn rate_scales_the_time_entering_the_clock() {
        let mut play = two_note_screen();
        play.advance(1_000_000);
        assert_eq!(play.now_us(), 1_000_000, "1× is unchanged");
        play.restart();
        play.set_rate(500);
        play.advance(1_000_000);
        assert_eq!(play.now_us(), 500_000);
    }

    #[test]
    fn speed_keys_step_through_the_presets_and_never_pass_1x() {
        let mut play = two_note_screen();
        for _ in 0..5 {
            play.nudge_rate(false);
        }
        assert_eq!(play.rate_permille(), 500, "stays at the slowest step");
        assert_eq!(play.nudge_rate(true), 625);
        // Off the steps (set over the socket): the nearest step that way.
        play.set_rate(700);
        assert_eq!(play.nudge_rate(false), 625);
        play.set_rate(700);
        assert_eq!(play.nudge_rate(true), 750);
        play.set_rate(1000);
        assert_eq!(play.nudge_rate(true), 1000, "keys never pass 1×");
    }

    #[test]
    fn set_rate_clamps_to_the_supported_range() {
        let mut play = two_note_screen();
        assert_eq!(play.set_rate(100), PLAY_RATE_MIN);
        assert_eq!(play.set_rate(5000), PLAY_RATE_MAX);
    }

    #[test]
    fn restart_keeps_the_speed() {
        let mut play = two_note_screen();
        play.set_rate(750);
        play.restart();
        assert_eq!(play.rate_permille(), 750);
    }

    #[test]
    fn the_backing_mutes_off_tempo_and_comes_back_at_the_fader_level() {
        let mut play = two_note_screen();
        let fader = Gain::new(0.6).unwrap();
        play.set_backing_gain(fader);
        play.set_rate(750);
        assert_eq!(play.applied_backing_gain(), Gain::SILENT);
        play.set_rate(1000);
        assert_eq!(play.applied_backing_gain(), fader);
    }

    #[test]
    fn a_slowed_loop_takes_proportionally_longer_to_count_in() {
        let mut play = loop_screen();
        play.set_rate(500);
        play.set_loop(1, 1);
        // One count-in bar of play time is two bars of wall time at 0.5×.
        play.advance(BAR);
        assert_eq!(play.now_us(), SHIFT + BAR / 2);
        assert!(matches!(phase(&play), Some(LoopPhase::CountIn { .. })));
        play.advance(BAR);
        assert_eq!(play.now_us(), SHIFT + BAR);
        assert_eq!(phase(&play), Some(LoopPhase::Demo));
    }

    #[test]
    fn the_speed_badge_shows_only_off_1x() {
        assert_eq!(speed_badge(1000), "");
        assert_eq!(speed_badge(750), "0.75×  ");
        assert_eq!(speed_badge(500), "0.5×  ");
        assert_eq!(speed_badge(625), "0.625×  ");
        assert_eq!(speed_badge(2000), "2×  ");
    }

    // ── scoring (M18-D) ──────────────────────────────────────────────────

    /// Run the take well past its end so every window has closed.
    fn run_out(play: &mut PlayScreen) {
        for _ in 0..20 {
            play.advance(1_000_000);
        }
    }

    #[test]
    fn a_perfect_take_scores_every_note() {
        let mut play = two_note_screen();
        play.advance(SHIFT);
        note_on(&mut play, 60);
        play.advance(1_000_000);
        note_on(&mut play, 62);
        run_out(&mut play);
        let s = *play.summary().expect("summary at the song end");
        assert_eq!((s.hits, s.misses, s.extras, s.perfect), (2, 0, 0, 2));
        assert_eq!(s.accuracy_bp, 10_000);
        // The desktop's award: 100 + 2·0, then 100 + 2·1.
        assert_eq!(s.score, 202);
        assert_eq!(s.best_combo, 2);
    }

    #[test]
    fn an_early_strike_inside_the_window_is_a_hit_not_perfect() {
        let mut play = two_note_screen();
        play.advance(SHIFT - 100_000);
        note_on(&mut play, 60);
        run_out(&mut play);
        let s = *play.summary().unwrap();
        assert_eq!((s.hits, s.perfect, s.early), (1, 0, 1));
        assert_eq!(s.score, 50, "a good hit earns 50 + combo");
    }

    #[test]
    fn a_miss_is_judged_once_its_window_closes_and_resets_the_combo() {
        let mut play = two_note_screen();
        play.advance(SHIFT);
        note_on(&mut play, 60);
        play.advance(200_000);
        assert_eq!(play.status().combo, 1);
        // D at SHIFT + 1 s is not played: still open just before its window
        // closes…
        play.advance(1_000_000 - 200_000 + ScoreConfig::default().good_us - 1);
        assert_eq!(play.status().misses, 0);
        // …a miss once it has.
        play.advance(1);
        let st = play.status();
        assert_eq!((st.hits, st.misses, st.combo, st.best_combo), (1, 1, 0, 1));
    }

    #[test]
    fn a_wrong_note_is_an_extra() {
        let mut play = two_note_screen();
        play.advance(SHIFT);
        note_on(&mut play, 61);
        run_out(&mut play);
        assert_eq!(play.summary().unwrap().extras, 1);
    }

    #[test]
    fn a_strike_during_a_wait_freeze_lands_on_the_awaited_note() {
        let mut play = two_note_screen();
        play.set_wait_mode(true);
        play.advance(SHIFT);
        play.advance(5_000_000); // frozen on C; device time runs on
        note_on(&mut play, 60);
        play.advance(1_000_000);
        note_on(&mut play, 62);
        run_out(&mut play);
        let s = play.summary().unwrap();
        assert_eq!((s.hits, s.perfect), (2, 2));
    }

    #[test]
    fn track_held_scores_the_same_as_ingest() {
        let mut play = two_note_screen();
        play.advance(SHIFT);
        play.track_held(NoteEvent::on(
            MidiNote::new(60).unwrap(),
            Velocity::new(80).unwrap(),
            0,
        ));
        run_out(&mut play);
        assert_eq!(play.summary().unwrap().hits, 1);
    }

    #[test]
    fn the_live_figures_after_the_last_note_equal_the_summary() {
        let mut play = loop_screen();
        // Play every other note (60, 62, 64, 66), 64 late.
        let advance_to = |play: &mut PlayScreen, t: u64| play.advance(t - play.now_us());
        for i in (0..8u64).step_by(2) {
            let late = if i == 4 { 100_000 } else { 0 };
            advance_to(&mut play, SHIFT + i * 1_000_000 + late);
            note_on(&mut play, 60 + i as u8);
        }
        advance_to(&mut play, SHIFT + 7_000_000);
        play.advance(ScoreConfig::default().good_us);
        let st = play.status();
        let s = play.finish();
        assert_eq!((st.hits, st.misses), (s.hits, s.misses));
        assert_eq!((st.score, st.best_combo), (s.score, s.best_combo));
        assert_eq!((s.hits, s.misses, s.late), (4, 4, 1));
    }

    #[test]
    fn seeking_back_unjudges_the_notes_after_the_target() {
        let mut play = two_note_screen();
        play.advance(SHIFT);
        note_on(&mut play, 60);
        play.advance(1_000_000);
        note_on(&mut play, 62);
        play.advance(500_000);
        assert_eq!(play.status().hits, 2);
        play.seek_to(SHIFT + 500_000);
        let st = play.status();
        assert_eq!((st.hits, st.misses), (1, 0), "D is played afresh");
    }

    #[test]
    fn restart_starts_a_fresh_take() {
        let mut play = two_note_screen();
        play.advance(SHIFT);
        note_on(&mut play, 60);
        run_out(&mut play);
        assert!(play.summary().is_some());
        play.restart();
        assert!(play.summary().is_none());
        let st = play.status();
        assert_eq!((st.score, st.hits, st.misses), (0, 0, 0));
    }

    #[test]
    fn the_summary_holds_the_finished_take_still() {
        let mut play = two_note_screen();
        run_out(&mut play);
        let at = play.now_us();
        play.advance(5_000_000);
        assert_eq!(play.now_us(), at);
    }

    /// Play the loop's two notes (62 at the loop start, 63 a second later)
    /// through one *your turn* pass; `with_63` drops the second.
    fn your_turn_pass(play: &mut PlayScreen, with_63: bool) {
        // Count-in → your turn lands exactly on the loop start.
        play.advance(BAR);
        assert_eq!(phase(play), Some(LoopPhase::YourTurn));
        note_on(play, 62);
        play.advance(1_000_000);
        if with_63 {
            note_on(play, 63);
        }
        play.advance(BAR); // the pass end jumps back to the count-in
    }

    #[test]
    fn the_loop_scores_only_your_turn_and_each_pass_on_its_own() {
        let mut play = loop_screen();
        play.set_loop(1, 1);
        // Demo: play along perfectly — nothing scores.
        play.advance(BAR);
        assert_eq!(phase(&play), Some(LoopPhase::Demo));
        note_on(&mut play, 62);
        play.advance(1_000_000);
        note_on(&mut play, 63);
        play.advance(BAR);
        assert_eq!(play.status().hits, 0, "demo strikes don't score");
        assert_eq!(play.loop_view().unwrap().last_pass, None);

        your_turn_pass(&mut play, true);
        let last = play.loop_view().unwrap().last_pass.unwrap();
        assert_eq!(
            last,
            PassSummary {
                pass: 1,
                hits: 2,
                misses: 0,
                accuracy_bp: 10_000
            }
        );
        assert_eq!(play.status().hits, 0, "the next pass starts from zero");
        assert_eq!(
            loop_badge(&play.loop_view().unwrap()),
            " Loop 2 · COUNT-IN · pass 2 · last 100% "
        );

        // Demo again, then a pass with one miss.
        play.advance(BAR);
        play.advance(BAR);
        your_turn_pass(&mut play, false);
        let last = play.loop_view().unwrap().last_pass.unwrap();
        assert_eq!(
            last,
            PassSummary {
                pass: 2,
                hits: 1,
                misses: 1,
                accuracy_bp: 5_000
            }
        );
    }

    #[test]
    fn stopping_the_loop_restores_whole_take_scoring() {
        let mut play = loop_screen();
        play.advance(SHIFT);
        note_on(&mut play, 60);
        play.advance(500_000);
        assert_eq!(play.status().hits, 1);
        play.set_loop(2, 2);
        assert_eq!(play.status().hits, 0, "the loop scores its own pass");
        play.clear_loop();
        assert_eq!(play.status().hits, 1, "the take's score is back");
    }

    #[test]
    fn numbers_group_in_threes() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(12_340), "12 340");
        assert_eq!(group_thousands(1_234_567), "1 234 567");
    }
}
