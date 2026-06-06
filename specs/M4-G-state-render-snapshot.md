# M4-G — tui+control: state + render-buffer snapshots for verification

> Milestone: M4 — Agent Interface · Issue: #91 · Suggested tier: sonnet
> Branch: `claude/m4-state-render-snapshot`

## Goal

Give an agent two ways to *verify* the result of its edits: a structured state
snapshot (already wired) and a **text dump of what's on screen** — the
deterministic, diffable "screenshot" that suits a TUI far better than pixels.
`query state` returns the `ComposerSnapshot`; `query render` returns the current
frame as text.

## Context

- Crates: `crates/tui` (renders to a buffer, serialises it) + `crates/control`
  (carries the `Render` response, M4-D #88). Builds on M4-F (#90) routing.
- `ratatui` already renders into a `Buffer` (the `tests/headless.rs` harness uses
  `TestBackend`, whose `buffer()` exposes the cell grid). Reuse that to render the
  current app frame off-screen and serialise it.

## What to do

- In `crates/tui`, add a method that renders the current screen to a `String`:

  ```rust
  // render the app's current view into a TestBackend of the live terminal size
  // and flatten the Buffer to text (one line per row; trailing blanks trimmed).
  pub fn render_to_string(&self, width: u16, height: u16) -> String;
  ```

  One line per buffer row, cells joined by their symbol; this is the agent's
  visual check ("is the note where I placed it on the highway?").
- Wire the two queries through the M4-F channel:
  - `Query::State` → `Response::Ok { state: Some(composer.snapshot()), .. }`.
  - `Query::Render` → `Response::Render { text: app.render_to_string(w, h) }`,
    using the app's current terminal dimensions (fall back to a sane default in
    headless mode).
- Keep `ComposerSnapshot` (M4-B) the single source for `state`; don't duplicate
  its fields here.

## Tests

- `render_to_string` on a composer with a couple of placed notes contains the
  keyboard row and a non-blank highway region; placing a note at a known cell
  changes the dump deterministically (same input → same string).
- Through the channel harness (M4-F style): `query state` returns a snapshot
  whose `notes` match what was added; `query render` returns non-empty text whose
  size matches the requested dimensions.

## Scope boundaries (do NOT)

- No pixel/PNG capture here — that is M4-H. Text only.
- Do not change the protocol enum shape (M4-D) beyond filling `Render`'s `text`.
  Do not change `core`.
- No new third-party deps (reuse `ratatui`'s `TestBackend`).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m4-state-render-snapshot`, `Closes #91`
