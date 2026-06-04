# M3-O — tui: region select + copy / paste / duplicate

> Milestone: M3 — Composer · Issue: #63 · Suggested tier: sonnet
> Branch: `claude/m3-region-select`

## Goal

Operate on many notes at once: select a region (pitch range × time range),
then copy, paste at the cursor, duplicate, or delete the selection — the bulk
editing that makes composing longer parts practical.

## Context

- Crate: `crates/tui` (`edit.rs`), with small pure helpers in `core::Timeline`
  (#49) for region queries and offset insertion so the selection math is
  CI-tested headlessly.
- Builds on the cursor (#52) and single-note ops (#53). Pairs with undo
  (#61): a paste/bulk-delete should be one undo step.

## What to do

Core helpers (pure, in `timeline.rs`):

```rust
/// Ids of notes whose start falls within [pitch_lo..=pitch_hi] x [us_lo..us_hi).
pub fn notes_in_region(&self, pitch_lo: u8, pitch_hi: u8, us_lo: u64, us_hi: u64) -> Vec<NoteId>;
/// Insert clones of `notes` shifted by (d_pitch semitones, d_us). Out-of-range
/// pitches are dropped. Returns the new ids.
pub fn insert_shifted(&mut self, notes: &[Note], d_pitch: i8, d_us: u64) -> Vec<NoteId>;
```

tui (append to #52 keymap):

| Key   | Action                                                      |
|-------|-------------------------------------------------------------|
| `v`   | start/extend visual selection from cursor                   |
| `y`   | yank (copy) selected notes to an in-editor clipboard         |
| `p`   | paste clipboard at the cursor (pitch/time offset from anchor)|
| `D`   | delete selection                                            |
| `Esc` | clear selection                                             |

- Selection is a rectangle anchored where `v` was pressed to the current cursor;
  highlight it in `draw`. Clipboard stores `Vec<Note>` normalised to the
  selection's top-left so paste offsets cleanly.
- Expose `selection_ids()` / `clipboard_len()` for tests.

## Tests

- core: `notes_in_region` returns exactly the notes inside the rectangle (edge
  inclusivity pinned); `insert_shifted` offsets and drops out-of-range pitches.
- tui: `v` + move + `y` copies the right count; `p` at a new cursor inserts that
  many notes at the offset; `D` removes the selection.

## Scope boundaries (do NOT)

- No cross-session clipboard; in-memory only.
- No resize-by-selection; move/copy/paste/delete only for v1.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-region-select`, `Closes #63`
