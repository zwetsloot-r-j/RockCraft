# M3-N — tui: keybinding / help overlay

> Milestone: M3 — Composer · Issue: #62 · Suggested tier: cheap
> Branch: `claude/m3-help-overlay`

## Goal

A discoverable cheat-sheet for the vim-style editor: `?` toggles an overlay
listing the keymap, so the user never has to memorise (or read the source) to
find a binding.

## Context

- Crate: `crates/tui`, extends `edit.rs` (#52). The authoritative keymap table
  lives in the #52 spec/comment and is extended by E/F/I/J/K/M/O — this overlay
  renders the same set.
- ratatui modal pattern: render the editor, then a centred `Block` popup over it
  (a `Clear` + bordered paragraph). No new deps.

## What to do

- `EditScreen` gains a `show_help: bool`. `?` toggles it; `Esc` (or `?` again)
  closes it. While shown, the overlay captures `Esc`/`?` and otherwise the editor
  keeps working (or freezes — pick and document; freezing is simpler).
- `draw`: when `show_help`, render a centred panel listing the bindings grouped
  by category (navigation, edit, chord, snap, transport, record, undo). Keep the
  list in one place so adding a binding updates the overlay.
- Expose `help_visible()` for tests.

## Tests (headless)

- `?` sets `help_visible()` true; `?`/`Esc` clears it.
- With help shown, the rendered `TestBackend` buffer contains a known binding
  string (e.g. "undo").

## Scope boundaries (do NOT)

- Do not invent bindings; reflect the ones defined by the other M3 tasks (it's
  fine to merge as those land, or list the currently-implemented subset).
- No scrolling/paging unless the list overflows; keep it one screen for v1.
- No new third-party deps.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m3-help-overlay`, `Closes #62`
