// Typed TypeScript mirrors of the `rockcraft-core` IPC payloads.
//
// Source of truth: `crates/core/src/composer.rs` (ComposerSnapshot, NoteView,
// Cursor, SelectionView, InputMode), `crates/core/src/grid.rs` (TimeSig,
// Subdivision), and `crates/core/src/action.rs` (Effect, the action catalog).
// These are field-for-field mirrors of the serde output; keep them in sync when
// the Rust structs change. A drift shows up as a `tsc` error at the call sites.

/** A `(pitch, step)` editing cursor — mirror of `composer::Cursor`. */
export interface Cursor {
  /** MIDI note in the 88-key range 21..=108. */
  pitch: number;
  /** Grid-step index along the time axis. */
  step: number;
}

/** A time signature — mirror of `grid::TimeSig`. */
export interface TimeSig {
  beats_per_bar: number;
  beat_unit: number;
}

/**
 * Grid subdivision — mirror of `grid::Subdivision`. Serde serialises the enum
 * as its variant name (no rename), so these are the exact wire strings.
 */
export type Subdivision =
  | "Quarter"
  | "Eighth"
  | "Sixteenth"
  | "ThirtySecond"
  | "EighthTriplet"
  | "SixteenthTriplet";

/**
 * How notes get into the editor — mirror of `composer::InputMode`. Serde
 * serialises the enum as its variant name.
 */
export type InputMode = "DirectEdit" | "StepRecord" | "LiveRecord";

/** One note in a snapshot — mirror of `composer::NoteView`. */
export interface NoteView {
  /** Stable note id (raw `NoteId` value). */
  id: number;
  pitch: number;
  start_us: number;
  dur_us: number;
  velocity: number;
}

/** The active selection rectangle — mirror of `composer::SelectionView`. */
export interface SelectionView {
  pitch_lo: number;
  pitch_hi: number;
  us_lo: number;
  us_hi: number;
}

/**
 * A read-only snapshot of the composer — mirror of `composer::ComposerSnapshot`.
 * Everything the frontend needs to draw, without exposing core internals.
 */
export interface ComposerSnapshot {
  notes: NoteView[];
  cursor: Cursor;
  bpm: number;
  /**
   * Grid phase origin (µs): the song time bar 1 / beat 1 / step 0 lands on.
   * Absent/0 for a grid that starts at song time 0. Bar/beat gridlines and the
   * cursor position are phased by this so they align to the performance.
   */
  grid_origin_us?: number;
  time_sig: TimeSig;
  subdivision: Subdivision;
  input_mode: InputMode;
  playing: boolean;
  playhead_us: number;
  looping: boolean;
  loop_start_us: number;
  loop_end_us: number;
  metronome: boolean;
  selection: SelectionView | null;
  chord_preview: number[] | null;
  clipboard_len: number;
  /** Backing-track alignment offset (`audio_start_us`); 0 when none. */
  backing_offset_us: number;
  /** Whether note-by-note wait mode ("pause on note") is armed. */
  wait_mode?: boolean;
  /**
   * Whether the transport is frozen on an unsatisfied wait step. `playing` stays
   * `true` while frozen (so the highway anchors on the playhead); the frontend
   * treats `playing && frozen` as a pause — holding the highway, the backdrop
   * video, and the backing audio until the awaited note is played.
   */
  frozen?: boolean;
  /** While frozen, the MIDI pitches to strike to advance; null otherwise. */
  awaiting?: number[] | null;
  /**
   * Background image layers, back-to-front, each transform **already evaluated**
   * at the playhead by `core` (M14-D). Absent/empty for pieces without any.
   */
  backgrounds?: BackgroundView[];
  /** Index of the layer background actions address; null when there are none. */
  selected_background?: number | null;
}

/**
 * Where a background image sits on the play surface — mirror of
 * `background::Transform`. Normalised, resolution-independent surface units:
 * `x`/`y` offset the image centre by that fraction of the surface
 * width/height, `scale` multiplies an `object-fit: contain` fit, and
 * `rotation_deg` turns it clockwise. `core` computes these; the webview only
 * renders them (see {@link cssTransform}).
 */
export interface BackgroundTransform {
  x: number;
  y: number;
  scale: number;
  rotation_deg: number;
  opacity: number;
}

/** The curve leaving a keyframe — mirror of `background::Easing`. */
export type BackgroundEasing =
  | "linear"
  | "ease_in"
  | "ease_out"
  | "ease_in_out"
  | "hold";

/** One pinned (song time, transform) pair — mirror of `background::Keyframe`. */
export interface BackgroundKeyframe {
  time_us: number;
  transform: BackgroundTransform;
  easing: BackgroundEasing;
}

/**
 * One background image layer in a {@link ComposerSnapshot} — mirror of
 * `background::BackgroundView`. `transform` is evaluated at the playhead.
 */
export interface BackgroundView {
  index: number;
  id: string;
  file: string;
  selected: boolean;
  transform: BackgroundTransform;
  keyframes: BackgroundKeyframe[];
}

/**
 * A side effect a frontend must carry out after applying an action — mirror of
 * `action::Effect`. Serde tags the variant under `"effect"` with snake_case
 * names; audio is not yet wired (#166/#167), the bridge only delivers these.
 */
export type Effect =
  | { effect: "audition_note"; pitch: number; velocity: number }
  | { effect: "audition_chord"; pitches: number[] }
  | { effect: "all_off" };

/**
 * The result of a successful `run_action` — mirror of `state::ActionReply`.
 *
 * `dirty` mirrors the backend's `AppState::dirty` flag: true when the timeline
 * has unsaved changes since the last save or load.
 */
export interface ActionReply {
  effects: Effect[];
  snapshot: ComposerSnapshot;
  dirty: boolean;
}

/**
 * Where to save a bundle — mirror of `state::SaveDest`.
 * Serde serializes as a tagged union (`kind` tag, snake_case).
 */
export type SaveDest =
  | { kind: "quick_save" }
  | { kind: "library"; name: string }
  /** Overwrite the loaded / last-saved bundle in place (no name prompt). */
  | { kind: "in_place" };

/**
 * Result of a successful `save_bundle` — the bundle directory as a string.
 * On error the command rejects with a string message.
 */
export type SaveBundleResult = string;

/**
 * One kept part for `split_bundle` — mirror of `state::SplitSegment` (itself the
 * mirror of `rockcraft_control::SegmentSpec`). The half-open song-time range
 * `[start_us, end_us)` is written as a new library bundle named `name`.
 */
export interface SegmentSpec {
  start_us: number;
  end_us: number;
  name: string;
}

/** One action parameter — mirror of `action::ParamInfo`. */
export interface ParamInfo {
  name: string;
  /** Rust scalar type name (`"u8"`, `"u64"`, `"i64"`, `"bool"`, …). */
  ty: string;
}

/** Self-describing metadata for one action — mirror of `action::ActionInfo`. */
export interface ActionInfo {
  name: ActionName;
  params: ParamInfo[];
  description: string;
}

/**
 * Every action name, as a string-literal union. Generated by hand from
 * `action_help()` / `action_names()`.
 *
 * SOURCE OF TRUTH: `crates/core/src/action.rs` (`action_names`). Keep this list
 * exactly in sync — adding a core action means adding its snake_case name here.
 */
export type ActionName =
  // ── navigation ──────────────────────────────────────────────────────
  | "cursor_left"
  | "cursor_right"
  | "cursor_up"
  | "cursor_down"
  | "cursor_bar_left"
  | "cursor_bar_right"
  | "cursor_octave_down"
  | "cursor_octave_up"
  | "cursor_to_start"
  | "cursor_to_end"
  | "cursor_to_pitch_min"
  | "cursor_to_pitch_max"
  | "set_cursor"
  | "subdivision_finer"
  | "subdivision_coarser"
  // ── edit ────────────────────────────────────────────────────────────
  | "add_note"
  | "delete_note"
  | "resize_note"
  | "adjust_velocity"
  | "toggle_grab"
  // ── tempo ─────────────────────────────────────────────────────────────
  | "adjust_bpm"
  | "set_bpm"
  // ── chord selector ──────────────────────────────────────────────────
  | "enter_chord_mode"
  | "commit_chord"
  | "cancel_chord"
  | "toggle_chord_kind"
  | "set_chord_degree"
  | "cycle_chord_degree"
  // ── input mode ──────────────────────────────────────────────────────
  | "toggle_record_arm"
  | "toggle_record_flavour"
  // ── wait mode ───────────────────────────────────────────────────────
  | "toggle_wait_mode"
  | "set_wait_mode"
  // ── transport ───────────────────────────────────────────────────────
  | "toggle_play_cursor"
  | "play_from_start"
  | "stop"
  | "play"
  | "set_playhead"
  // ── backing alignment ───────────────────────────────────────────────
  | "nudge_backing_offset"
  // ── loop / metronome / count-in ─────────────────────────────────────
  | "toggle_loop"
  | "toggle_metronome"
  | "start_count_in_record"
  | "set_loop_bounds"
  | "set_loop_start"
  | "set_loop_end"
  // ── selection / clipboard ───────────────────────────────────────────
  | "start_selection"
  | "clear_selection"
  | "yank_selection"
  | "paste_clipboard"
  | "delete_selection"
  // ── background images (M14-D) ───────────────────────────────────────
  | "select_background"
  | "cycle_background"
  | "nudge_background_pos"
  | "nudge_background_scale"
  | "nudge_background_rotation"
  | "set_background_opacity"
  | "set_background_easing"
  | "add_background_keyframe"
  | "delete_background_keyframe"
  // ── history ─────────────────────────────────────────────────────────
  | "undo"
  | "redo";

/** JSON params object for an action invocation. */
export type ActionParams = Record<string, unknown>;

/**
 * One row in the library browser — mirror of `library::LibraryEntryDto`.
 * Source of truth: `tauri-app/src-tauri/src/library.rs`.
 */
export interface LibraryEntryDto {
  /** Display name — bundle directory's file name. */
  name: string;
  /** Absolute path to the bundle directory as a UTF-8 string. */
  dir: string;
  /** Number of notes in `song.mid`. */
  note_count: number;
  /** Total chart duration in microseconds. */
  duration_us: number;
  /**
   * Origin label: `"recorded"`, `"composed"`, `"edited"`, `"imported"`, or
   * `null` for legacy bundles without a `meta.json`.
   */
  origin: string | null;
  /** Whether the bundle declares a backing audio track. */
  has_backing: boolean;
}

// ── Play screen (#168) ──────────────────────────────────────────────────────

/**
 * One projected note span for the highway — mirror of `play::SpanView`.
 * Bounds are in MILLISECONDS (the webview engine is ms-based), already shifted
 * by the pre-roll. No hand info exists in a bundle.
 */
export interface PlaySpan {
  note: number;
  /** Start in milliseconds. */
  start: number;
  /** End in milliseconds. */
  end: number;
}

/**
 * Static song info returned by `play_load` — mirror of `play::PlayInfo`.
 * Source of truth: `tauri-app/src-tauri/src/play.rs`.
 */
export interface PlayInfo {
  title: string;
  notes: PlaySpan[];
  /** Whole-song forward shift in microseconds (`song_shift_us`). */
  shift_us: number;
  /** Total song length including the lead-in, in microseconds. */
  duration_us: number;
  /** Lead window the highway shows top→hit-line, in microseconds. */
  lead_us: number;
  has_backing: boolean;
  /**
   * Background video to render behind the highway, or `null` when the piece has
   * no backdrop (M9-G). Mirror of `play::BackgroundVideoView`.
   */
  video: BackgroundVideoView | null;
  /**
   * Background image layers to render behind the highway, back-to-front (M14-D).
   * The *static* half — the moving transforms arrive per tick in
   * {@link PlayStateEvent}. Mirror of `play::BackgroundLayerView`.
   */
  backgrounds: BackgroundLayerView[];
  hear_song: boolean;
  /** Piece tempo (BPM) for the highway bar/beat grid; 120 when no grid. */
  bpm: number;
  /** Beats per bar (time-signature numerator); 4 when no grid. */
  beats_per_bar: number;
}

/**
 * A background image layer returned with {@link PlayInfo} — mirror of
 * `play::BackgroundLayerView`.
 */
export interface BackgroundLayerView {
  id: string;
  /** Absolute file path (wrap with `convertFileSrc` before assigning to an `<img>`). */
  path: string;
}

/**
 * One layer's transform at the current song time — mirror of
 * `play::BackgroundTransformView`. Evaluated by `core` each tick.
 */
export interface BackgroundTransformView {
  id: string;
  transform: BackgroundTransform;
}

/**
 * A background video reference returned with {@link PlayInfo} — mirror of
 * `play::BackgroundVideoView` (and `state::VideoRef`).
 */
export interface BackgroundVideoView {
  /** Absolute file path (wrap with `convertFileSrc` before assigning to a `<video>`). */
  path: string;
  /** Alignment offset in microseconds (`videoTime = songTime + offset_us`). */
  offset_us: number;
}

/**
 * A ~60 Hz live snapshot pushed while a take runs — mirror of
 * `play::PlayStateEvent`.
 */
export interface PlayStateEvent {
  /** Current song time in microseconds (the authoritative clock). */
  time_us: number;
  /** Whether wait mode has frozen the clock. */
  frozen: boolean;
  score: number;
  combo: number;
  best_combo: number;
  hits: number;
  misses: number;
  /** Notes currently held by the player. */
  held: number[];
  /** Notes the player must hold to un-freeze (empty unless `frozen`). */
  awaiting: number[];
  /**
   * Notes judged since the previous snapshot, in song-time order (M14-B).
   * One-shot: a judged note appears in exactly one `play_state`, so the screen
   * spawns one decaying effect per entry without de-duplicating.
   */
  judgments: HitFeedback[];
  /**
   * Each background layer's transform at this instant, in the same order as
   * `PlayInfo.backgrounds` (M14-D). Empty when the piece has none.
   */
  backgrounds: BackgroundTransformView[];
  /** Set once the song (plus tail) has finished. */
  finished: boolean;
}

/**
 * How loud a judged note should read — mirror of `core::scoring::Feedback`
 * (`as_str`). The strength rule lives in core; the frontend only maps a level
 * onto pixels.
 */
export type FeedbackLevel = "clear" | "near" | "subtle";

/** Timing detail behind a judgment — mirror of `core::scoring::Timing` + miss. */
export type FeedbackTiming = "perfect" | "early" | "late" | "miss";

/** One judged note — mirror of `play::HitFeedbackView`. */
export interface HitFeedback {
  /** Pitch, i.e. which lane the effect belongs at. */
  note: number;
  level: FeedbackLevel;
  timing: FeedbackTiming;
  /** Signed timing error in microseconds (negative = early); 0 on a miss. */
  error_us: number;
  /** The (shifted) song time of the note this judges. */
  time_us: number;
}

/** End-of-take summary returned by `play_finish` — mirror of `play::PlaySummary`. */
export interface PlaySummary {
  total_expected: number;
  hits: number;
  misses: number;
  extras: number;
  perfect: number;
  early: number;
  late: number;
  /** Accuracy in basis points (0..=10000); divide by 100 for a percentage. */
  accuracy_bp: number;
  best_combo: number;
  score: number;
}
