# M7-tauri-C-library — Library browser screen (+ shared bundle scanning)

> Milestone: M7 · Issue: #163 · Suggested tier: sonnet
> Branch: `claude/tauri-library`
> Depends on: M7-tauri-A-ipc-bridge (#161), M7-tauri-B-shell-menu (#162)

## Goal

Port the library browser: list every saved/recorded/imported bundle and open a
selection in Play or Edit. The bundle scanner moves out of the TUI crate so
both frontends share one implementation.

## Context

- TUI screen: `crates/tui/src/library_screen.rs` (`LibraryScreen`,
  `LibraryOutcome::{OpenPlay, OpenEdit, Cancelled, Pending}`).
- Scanner: `crates/tui/src/library.rs` — `LibraryEntry { name, dir,
  note_count, duration_us, origin, has_backing }`, `default_scan_roots()`
  (`~/.rockcraft/library`, `recordings/`, `import-out/`), `slug()`.
- The scanner does file I/O and parses `song.mid`/`meta.json`, so it cannot
  live in `core` (purity invariant). `crates/midi` already owns file
  parse/record (`file.rs`) — that's the home.

## What to do

### 1. Move the scanner to `crates/midi`

- New module `crates/midi/src/bundle.rs`: move `LibraryEntry`,
  `default_scan_roots`, `slug`, the scan function(s), and their unit tests
  verbatim from `crates/tui/src/library.rs` (adjust imports only).
- `crates/tui/src/library.rs` becomes a re-export (`pub use
  rockcraft_midi::bundle::*;`) so all TUI call sites compile unchanged.
- This is a move, not a rewrite — behaviour identical, tests come along.

### 2. Backend command (`tauri-app/src-tauri/`)

```rust
#[tauri::command]
fn scan_library() -> Vec<LibraryEntryDto>;
// DTO mirrors LibraryEntry with dir as String; derives Serialize
```

### 3. Frontend (`tauri-app/src/screens/library/`)

`LibraryScreen.tsx`: dark list panel in the prototype style. One row per
bundle: name, origin badge (Recorded / Composed / Edited / Imported / `?` for
legacy `None`), note count, duration `M:SS`, and a dot when `has_backing`.
Empty state: "no recordings yet — Record or Compose from the menu".

Keys (match the TUI): ArrowUp/Down/`j`/`k` move, Enter or `p` → navigate to
`{ kind: "play", dir }`, `e` → `{ kind: "edit", dir }`, Esc/`q` → menu.
Mouse: click selects, double-click plays.

Until the live play/edit screens exist (#164/#168), those router variants may
render the Placeholder with the chosen `dir` shown — wire the navigation, not
the destination.

### 4. Menu hookup

"Play last recording" / "Edit last recording" stay as-is; "Library" is
reached by making the menu's Play/Edit items route through the library screen
when more than one bundle exists? **No** — keep TUI parity: add a `Library`
entry to the router and have "Play last recording" / "Edit last recording"
keep their TUI semantics. The library screen is navigated from the menu items
exactly as `crates/tui/src/app.rs::menu_activate()` does it — read that
function and copy its routing decisions.

## Tests

- Existing scanner tests pass from their new `crates/midi` home.
- New Rust test for the DTO conversion (entry → DTO preserves all fields).
- `npx tsc --noEmit` for the screen.

## Scope boundaries (do NOT)

- Do not change scanner behaviour (roots, ordering, legacy handling).
- Do not implement the play/edit destinations.
- Do not add delete/rename of bundles (not in the TUI either).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green (scanner tests now under `crates/midi`)
- [ ] `npx tsc --noEmit` passes
- [ ] TUI library browser unchanged in behaviour (`cargo run --bin rockcraft-tui`)
- [ ] `npm run dev`: library lists bundles from the scan roots with metadata;
      Enter/`p`/`e`/Esc behave as specified
- [ ] PR opened against `main` from `claude/tauri-library`, `Closes #163`
