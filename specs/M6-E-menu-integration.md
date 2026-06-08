# M6-E — tui: menu integration (Import from video file / URL)

> Milestone: M6 — Video Import · Issue: #117 · Suggested tier: sonnet
> Branch: `claude/m6-menu-integration`

## Goal

Make import reachable from the game itself: a menu entry that imports a
Synthesia video into a chart via the M6-D pipeline, shows progress, and on
success makes the chart playable/editable.

## Context

- Crate `crates/tui`. Adds `rockcraft-import` (M6-D, #116) as a dependency.
- Reuse the existing file-picker pattern in `crates/tui/src/backing.rs`
  (`BackingPicker`) for choosing a local video; reuse the menu/`Screen` plumbing
  in `app.rs` (`MENU_ITEMS`, `menu_activate`, `Screen`).
- The pipeline runs off-thread (like the control server) so it never blocks the
  render/MIDI loop; progress events (`import::Progress`) are polled and drawn.
- The URL option depends on M6-D's fetch hook being configured; otherwise it is
  hidden/disabled.

## What to do

1. **Menu items.** Add **"Import from video file…"** (always present) and
   **"Import from URL…"** (present/enabled only when a fetch command is
   configured — query M6-D for that). Place them sensibly in `MENU_ITEMS`.
2. **File flow.** "Import from video file…" opens a video file picker (reuse
   `BackingPicker`, filtered to video extensions). On select, start
   `import_video(File(path), …)` on a worker thread.
3. **URL flow.** "Import from URL…" opens a simple text-input overlay; on
   submit, start `import_video(Url(text), …)`.
4. **Progress screen.** A `Screen::Importing` that renders the latest
   `Progress` (Fetching / Extracting % / Writing). On `Done(bundle)`, load it
   into Play (or Edit) — reuse the existing bundle-load path in `app.rs`. On
   error, return to the menu with a clear status line (no sidecar set up / no
   fetch command / extraction failed).
5. **(Optional / may defer)** A minimal review of M6-C low-confidence notes
   before load — can be a follow-up issue if it bloats this one.

## Tests

- Headless (mock pipeline): driving the menu to the file-import item with a
  stubbed `import_video` advances through `Importing` to a loaded chart screen;
  the render snapshots the progress states.
- The URL menu item is **absent/disabled** when no fetch command is configured
  and **present** when one is (inject the capability flag).
- Errors render a status line and return to the menu.

## Scope boundaries (do NOT)

- No extraction / download / CV logic in the TUI — call `rockcraft-import`.
- Charts load only from the gitignored output dir (M6-B); never write under a
  tracked path.
- Do not block the render/MIDI loop on the pipeline.
- No new third-party deps beyond `rockcraft-import`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Host check: importing a (local synthetic) video lands a playable chart
- [ ] PR against `main` from `claude/m6-menu-integration`, `Closes #117`
