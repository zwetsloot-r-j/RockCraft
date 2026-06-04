# M3-M — core+tui: undo / redo

> Milestone: M3 — Composer · Issue: #61 · Suggested tier: sonnet
> Branch: `claude/m3-undo-redo`

## Goal

Every edit is reversible. Add an undo/redo history over `Timeline` mutations so
the composer can experiment freely. Bound the history so memory stays sane.

## Context

- Crates: `crates/core` (the history; pure + testable) and `crates/tui`
  (key bindings + wiring through #53/F edits).
- Builds on `Timeline` (#49). Keep it simple and robust: a **snapshot stack**
  of `Timeline` clones is acceptable for these sizes (a few hundred notes);
  avoids per-op inverse bookkeeping. A command/inverse design is allowed if the
  author prefers, but snapshots are the recommended v1.

## What to do

```rust
// crates/core/src/history.rs  (or fold into timeline.rs)
pub struct History {
    // bounded undo/redo stacks of Timeline snapshots
}
impl History {
    pub fn new(timeline: Timeline, capacity: usize) -> Self;
    pub fn current(&self) -> &Timeline;
    pub fn current_mut(&mut self) -> &mut Timeline; // for in-place edits
    /// Push the current state as a checkpoint *before* (or after) a mutation,
    /// clearing the redo stack. Document the chosen checkpoint discipline.
    pub fn checkpoint(&mut self);
    pub fn undo(&mut self) -> bool; // false if nothing to undo
    pub fn redo(&mut self) -> bool; // false if nothing to redo
}
```

- `EditScreen` holds a `History` instead of a bare `Timeline`; each user edit
  (add/delete/resize/move/velocity/chord-insert) takes a checkpoint so it undoes
  as one step. A chord insert (multiple notes) must undo as a single step.
- tui keymap (append to #52): `u` = undo, `Ctrl-r` (or `R`? avoid clash with
  #57 record-arm — pick and document) = redo.

## Tests (core, headless)

- After N edits, `undo` restores each previous state in order; `redo` replays.
- A new edit after undo clears the redo stack.
- History is bounded to `capacity` (oldest dropped); never panics on empty
  undo/redo (returns false).
- A multi-note checkpoint (chord) undoes/redoes as one step.

## Scope boundaries (do NOT)

- Do not change `Timeline`'s public op signatures (#49); wrap them.
- Resolve the redo-key vs record-arm-key clash with #57; document the final map.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-undo-redo`, `Closes #61`
