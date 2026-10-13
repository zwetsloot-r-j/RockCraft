/**
 * Screen discriminated union — mirrors the TUI `Screen` enum in
 * `crates/tui/src/app.rs`. Later issues extend variants with payloads
 * (e.g. bundle dir for library).
 */
export type Screen =
  | { kind: "menu" }
  /** Play mode. `dir` carries the bundle directory when opened from the library.
   * `atUs` (song µs) opens the take paused on the downbeat of the bar
   * containing it — Edit's "play here" (M24-B). */
  | { kind: "play"; dir?: string; atUs?: number }
  /**
   * The unified capture + edit screen (M9-A): both records (StepRecord /
   * LiveRecord input modes) and hand-edits the same timeline. `dir` carries the
   * bundle directory when continuing/opening an existing bundle; `armed` opens
   * it already in record mode (the "New piece" entry point). The standalone
   * `record` screen was retired into this one. `atUs` (song µs) puts the cursor
   * on the downbeat of the bar containing it — Play's "edit here" (M23-A).
   */
  | { kind: "edit"; dir?: string; armed?: boolean; atUs?: number }
  | { kind: "video-picker" }
  | { kind: "url-input" }
  | { kind: "importing" }
  | { kind: "library" };
