/**
 * Screen discriminated union — mirrors the TUI `Screen` enum in
 * `crates/tui/src/app.rs`. Later issues extend variants with payloads
 * (e.g. bundle dir for library).
 */
export type Screen =
  | { kind: "menu" }
  | { kind: "record" }
  | { kind: "play" }
  | { kind: "edit" }
  | { kind: "backing-picker" }
  | { kind: "video-picker" }
  | { kind: "url-input" }
  | { kind: "importing" }
  | { kind: "library" };
