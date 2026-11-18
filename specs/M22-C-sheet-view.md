# M22-C — Sheet view: show the piece as notation inside the desktop editor

> Milestone: M22 — Sheet-music export · Issue: #383 · Suggested tier: sonnet
> Branch: `claude/m22-sheet-view` · Depends on: M22-A (#329), M22-B (#330)

## Goal

See the piece as sheet music without leaving RockCraft. In the desktop editor,
one key opens a sheet view that draws the open piece — both staves, key, tempo
marks, finger numbers — and from there the user can still save the MusicXML
file or print.

## Context

- M22-B writes MusicXML (`rockcraft_export::to_musicxml`). This spec *draws* it,
  which M22-B ruled out for itself.
- Renderer: **OpenSheetMusicDisplay** (npm `opensheetmusicdisplay`, BSD-3-Clause),
  chosen over Verovio after a side-by-side comparison
  (`tools/notation-compare/index.html`): little visible difference, a fifth of
  the download size, a simpler licence, and plain JavaScript rather than
  WebAssembly. It reads MusicXML and draws SVG in the webview.
- Our export writes no beams. OSMD's `autoBeam` option adds them; use it.
- Editor overlays: `EditScreen.tsx` has an `Overlay` union and one `onKeydown`
  branch per overlay.
- The TUI cannot show engraved notation and gets no view.

## What to do

### 1. Host command `query_score` (both frontends)

`HostCommand::QueryScore` → `"query_score"`, no params. Returns
`{ musicxml, title }`: the MusicXML document `export_score` would write, as
text, without touching the disk. Fails with `nothing to export` on an empty
piece. Share the build with `export_score` — one function in `crates/export`
that returns the text, which `export_score_file` then writes.

- Tauri: `state::score_musicxml`, IPC command `edit_query_score`, the arm in
  `control.rs`.
- TUI: the arm in `crates/tui/src/app.rs` (it can answer; it just has no view).
- Catalog entry, names list and parity tests in `host.rs`;
  `docs/AGENT-CONTROL.md` row.

### 2. Dependency

Add `opensheetmusicdisplay` (pinned to an exact version) to
`tauri-app/package.json`. Load it with a dynamic `import()` the first time the
sheet view opens, so it is not part of the editor's start-up bundle.

### 3. Sheet view overlay (`tauri-app/src/screens/edit/`)

- **`N`** opens the sheet view (it replaces M22-B's direct export on that key;
  export moves inside the view).
- A new `SheetView.tsx` component: a full-editor overlay with a white,
  scrollable sheet. It fetches `edit_query_score`, renders with OSMD
  (`backend: "svg"`, `autoBeam: true`, `drawFingerings: true`,
  `drawPartNames: false`, title drawn), and shows a short message instead when
  the piece is empty or rendering fails.
- Keys while it is open — nothing else reaches the editor:

  | key | effect |
  | --- | --- |
  | `Esc`, `N` | close |
  | `+` / `=`, `-` | zoom in / out (re-render) |
  | `e` | export the MusicXML file (M22-B's save dialog and flash) |
  | `p` | print the sheet (`window.print()`, with print styles that show only the sheet) |

- The view is a snapshot taken when it opens; it does not follow edits (it
  cannot be edited behind the overlay anyway).
- Help overlay: `N` row becomes "Sheet view (notation); e exports, p prints".
- README "Printing sheet music": mention the view.

## Tests

- `host.rs` parity tests include `query_score`.
- `crates/export`: the text-returning function gives the same document
  `export_score_file` writes, and `NothingToExport` for an empty piece.
- Tauri crate: `score_musicxml` returns a document with the piece's notes and
  the `Untitled` title; an empty piece errors.
- Vitest: pure helpers only — the overlay's key map (`sheetKey`) and zoom
  clamping. No test renders OSMD (it needs a real browser layout).
- `tsc`, vitest and `vite build` stay green; the editor's main chunk must not
  contain OSMD (check the build output for a separate chunk).

**Manual check (local, not CI):** open a piece, press `N`, confirm staves, key,
tempo, beams and finger numbers; zoom; `e` writes a file; `p` opens the print
dialog showing only the sheet.

## Scope boundaries (do NOT)

- No editing in the sheet view, no playback cursor, no click-to-seek.
- No sheet view on the play screen or in the TUI.
- No PDF writer of our own (printing goes through the webview).
- No change to the notation model or the exporter's output.
- No second renderer; no CDN loading — the library is bundled.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Tauri crate tests and frontend `tsc`/vitest/build green locally
- [ ] PR opened against `main` from the branch above, `Closes #383`
