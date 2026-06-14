/**
 * Screen discriminated union — mirrors the TUI `Screen` enum in
 * `crates/tui/src/app.rs`. Later issues extend variants with payloads
 * (e.g. bundle dir for library).
 */
export type Screen =
  | { kind: "menu" }
  /** Play mode. `dir` carries the bundle directory when opened from the library. */
  | { kind: "play"; dir?: string }
  /**
   * The unified capture + edit screen (M9-A): both records (StepRecord /
   * LiveRecord input modes) and hand-edits the same timeline. `dir` carries the
   * bundle directory when continuing/opening an existing bundle; `armed` opens
   * it already in record mode (the "New piece" entry point). The standalone
   * `record` screen was retired into this one.
   */
  | { kind: "edit"; dir?: string; armed?: boolean }
  | { kind: "backing-picker" }
  | { kind: "video-picker" }
  | { kind: "url-input" }
  | { kind: "importing" }
  | { kind: "library" };
