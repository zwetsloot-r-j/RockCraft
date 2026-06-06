# M4-C — tui: `EditScreen` delegates to `core::Composer`

> Milestone: M4 — Agent Interface · Issue: #87 · Suggested tier: opus
> Branch: `claude/m4-tui-action-refactor`

## Goal

Re-seat the TUI on the pure `Composer` (M4-B). `EditScreen` keeps only what is
genuinely a frontend concern — a `KeyCode → Action` keymap, an `Effect →
synth` interpreter, view-only state (help overlay), file save, and rendering.
All editing logic now comes from `core`. This decouples view from logic and
makes key-rebinding a table edit instead of a code change.

## Context

- Crate: `crates/tui`, rewrites `edit.rs` internals. Builds on `Composer`,
  `Action`, `Effect` from M4-A/M4-B (#85/#86).
- **No behaviour change.** The existing tests in `crates/tui` (and
  `tests/key_driven.rs`, `tests/headless.rs`) are the contract. Do **not**
  modify, weaken, or delete them — they must pass unchanged. Keep the public
  read accessors (`note_count`, `cursor`, `note_under_cursor`, `is_playing`,
  `previewed_chord`, …) as thin shims over `Composer`.

## What to do

- `EditScreen` holds a `Composer` (replacing the inline `history/grid/cursor/
  grabbed/chord/selection/clipboard/transport/loop/metronome/count-in` fields),
  plus frontend-only fields: `synth: Option<SynthHandle>`, the audition
  bookkeeping (`auditioning`, `auditioning_chord` — the "currently sounding"
  set), and `show_help`.
- Add a keymap function mapping `KeyCode` → `Option<Action>`, e.g.:

  ```rust
  fn key_to_action(code: KeyCode /*, mode flags */) -> Option<Action>;
  ```

  `on_key` becomes: handle `?`/help locally; otherwise
  `if let Some(a) = key_to_action(code) { let fx = self.composer.apply(a); self.run_effects(fx); }`.
  Chord-mode key routing (`on_chord_key`) maps to the chord `Action`s.
- `run_effects(Vec<Effect>)` interprets each effect against the synth, owning the
  stop-previous-then-play discipline that `audition`/`audition_chord` do today.
  `AllOff` silences everything and clears the bookkeeping.
- The run loop calls `composer.advance(dt_us)` (replacing `tick_audition`) and
  feeds the returned effects through `run_effects`. Played MIDI goes through
  `composer.ingest(ev)` and likewise.
- `save`/`save_bundle` stay in the TUI and read `composer.timeline()` + grid/key.
- Rendering reads `composer` accessors / `snapshot()`; no logic in the draw path.
- Document in the module header that the keymap table is the rebinding seam
  (full user-facing rebind UI is out of scope here).

## Tests

- All pre-existing `crates/tui` tests pass **unchanged**.
- Add a focused test that a representative key (`a`) routes through
  `key_to_action` → `Action::AddNote` → `Composer` and that the resulting
  `Effect::AuditionNote` is consumed (synth optional / `None` is a no-op).

## Scope boundaries (do NOT)

- Do not change `core` signatures (consume M4-A/B as-is). Do not alter existing
  tests. Do not add the WebSocket here (M4-D+).
- Do not introduce new editor behaviour or new key bindings beyond what exists.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green (including unchanged TUI tests)
- [ ] PR against `main` from `claude/m4-tui-action-refactor`, `Closes #87`
