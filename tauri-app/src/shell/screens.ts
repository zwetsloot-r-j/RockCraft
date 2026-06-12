/**
 * Screen discriminated union — mirrors the TUI `Screen` enum in
 * `crates/tui/src/app.rs`. Later issues extend variants with payloads
 * (e.g. bundle dir for library).
 */
export type Screen =
  | { kind: "menu" }
  | { kind: "record" }
  /** Play mode. `dir` carries the bundle directory when opened from the library. */
  | { kind: "play"; dir?: string }
  /** Edit mode. `dir` carries the bundle directory when opened from the library. */
  | { kind: "edit"; dir?: string }
  | { kind: "backing-picker" }
  | { kind: "video-picker" }
  | { kind: "url-input" }
  | { kind: "importing" }
  | { kind: "library" };
