# M3-D — tui: Edit screen + 88-lane cursor navigation

> Milestone: M3 — Composer · Issue: #52 · Suggested tier: opus
> Branch: `claude/m3-edit-screen`

## Goal

Add a third screen, `Screen::Edit`, that renders a `Timeline` on the existing
note-highway projection with a movable `(pitch, step)` cursor navigated vim-style
across all 88 keys. This is the shell every other TUI composer task plugs into.
**Navigation + rendering only** — note mutation is #53.

## Context

- Crate: `crates/tui` (new module `edit.rs`; wire into `app.rs`). Depends on
  `core::Timeline` (#49) and `core::Grid` (#50).
- Reuse, do not reinvent: `highway::{build_spans, project}` for vertical (time)
  placement; `keyboard::{Scale, white_key_slots, black_key_col, is_black_key,
  LOWEST_MIDI, HIGHEST_MIDI}` for horizontal (pitch) placement. Cursor time
  position = `grid.us_of_step(cursor.step)`.
- Test harness already exists: `key_source::ScriptedKeys` + ratatui `TestBackend`
  (see `tui/tests/key_driven.rs`, `app::run_loop`). The screen must be headless-
  testable the same way.

## What to do

```rust
// crates/tui/src/edit.rs
pub struct EditScreen {
    timeline: Timeline,
    grid: Grid,
    cursor: Cursor, // { pitch: u8 (21..=108), step: u64 }
    // viewport scroll state for pitch + time so the cursor stays visible
}
impl EditScreen {
    pub fn new() -> Self;                 // empty timeline, Grid::default_120, cursor mid-keyboard
    pub fn from_timeline(t: Timeline, g: Grid) -> Self;
    pub fn cursor(&self) -> Cursor;       // for tests
    pub fn on_key(&mut self, code: KeyCode); // navigation routing
    pub fn draw(&self, f: &mut Frame, area: Rect);
}
```

**Navigation keymap** (this screen owns key routing; later tasks E/F/I/J/K/M/N/O
*extend* this table — keep it documented here as the authoritative list):

| Key            | Action                                  |
|----------------|-----------------------------------------|
| `h` / `←`      | cursor left one step (clamp at 0)       |
| `l` / `→`      | cursor right one step                   |
| `j` / `↓`      | cursor down one semitone (clamp A0=21)  |
| `k` / `↑`      | cursor up one semitone (clamp C8=108)   |
| `H` / `L`      | jump left/right one bar                 |
| `J` / `K`      | jump down/up one octave (clamp)         |
| `0` / `$`      | cursor to song start / last note end    |
| `Tab` / `Esc`  | back to menu (handled by `Shell`)       |

**Rendering:** draw the timeline notes via `project` against a fixed lead window
(e.g. a few bars derived from `grid.bar_us()`), the 88-key keyboard along the
bottom (existing geometry), beat/bar gridlines, and the cursor cell highlighted.
When the board is wider than the area, horizontally scroll to keep the cursor
column visible. Status line shows bar:beat, snap label, cursor pitch name.

Wire into `app.rs`: add `Screen::Edit(EditScreen)` to the enum, `screen_name()`,
`draw`, and `on_key` routing (Tab/Esc → menu; other keys → `edit.on_key`). A menu
entry to *enter* the editor is #55; for now a test can construct the screen
directly or add a temporary entry.

## Tests

- Fresh `EditScreen`: cursor starts in a defined place; `h` at step 0 stays at 0.
- `l` then `h` returns to the start step; `k`/`j` move pitch by 1 and clamp at
  108/21; `J`/`K` move by 12 and clamp.
- `H`/`L` move by exactly `grid.bar_us()/grid.step_us()` steps.
- A `from_timeline` screen renders without panic on a `TestBackend` and the
  rendered buffer contains a known note's pitch marker (headless, via existing
  harness).

## Scope boundaries (do NOT)

- No note add/remove/resize here (that is #53); unmapped keys are no-ops.
- Do not modify `highway.rs`/`keyboard.rs` public APIs; consume them.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-edit-screen`, `Closes #52`
