# M7-tauri-E-edit-overlays — Chord selector, help overlay, save/load bundles

> Milestone: M7 · Issue: #165 · Suggested tier: sonnet
> Branch: `claude/tauri-edit-overlays`
> Depends on: M7-tauri-D-edit-grid (#164)

## Goal

Finish the edit screen to TUI parity: the chord-selector overlay, the help
overlay, the dirty-exit and save-as prompts, and real bundle persistence
(`save_bundle` / `load_bundle` backend commands using the same format the TUI
writes).

## Context

- TUI reference: `crates/tui/src/edit.rs` — `chord_key_to_action()`, the help
  overlay text, `exit_prompt` / `name_prompt` handling, and the save path;
  `crates/tui/src/app.rs::open_edit_from_midi()` for loading.
- Bundle format: directory with `song.mid` + `meta.json`
  (`core::song::RecordingMeta { midi_file, backing, grid, key, origin }`) +
  optional backing audio copy. Save targets: `recordings/take-<timestamp>/`
  (quick save) or `<library_root>/<slug>/` (save-as; `slug()` from the
  scanner module, in `crates/midi` after #163).
- MIDI bytes: `crates/midi` `events_to_smf_bytes` (used by the TUI save) and
  the parse used by `open_edit_from_midi`.
- Chord actions already exist in core: `enter_chord_mode`, `commit_chord`,
  `cancel_chord`, `toggle_chord_kind`, `set_chord_degree {degree}`,
  `cycle_chord_degree {delta}`; the preview pitches arrive in
  `snapshot.chord_preview`.

## What to do

### 1. Chord selector (frontend only)

When `snapshot.chord_preview` is non-null, show a floating selector panel
near the cursor: current degree (1–7 with roman numeral), kind
(Triad/Seventh), preview pitch names. Keys (match
`chord_key_to_action()`): `1`–`7` set degree, `]`/`[` cycle ±1, `s` toggle
kind, Enter commit, Esc cancel. Remove the #164 stopgap key-swallowing.

### 2. Help overlay

`?` toggles a scrollable overlay listing every binding with its description.
Generate the list from the #164 keymap table + chord table (single source) —
do not hand-write a second copy. Esc or `?` closes.

### 3. Save / load (backend commands)

```rust
#[tauri::command]
fn save_bundle(state: State<AppState>, dest: SaveDest) -> Result<String, String>;
// SaveDest::QuickSave | SaveDest::Library { name: String }
// Writes song.mid from the composer timeline + meta.json (grid, key, origin),
// copies the attached backing file if any. Returns the bundle dir.

#[tauri::command]
fn load_bundle(state: State<AppState>, dir: String) -> Result<ComposerSnapshot, String>;
// Parses song.mid (+ meta.json if present) and replaces the composer via
// Composer::from_timeline(timeline, grid) + set_key/origin bookkeeping —
// mirror open_edit_from_midi(), including the legacy no-meta fallback.
```

Track `dirty` in the backend (`AppState`): set on any mutating `run_action`,
cleared on save; expose it in the reply or a `dirty` event so the UI shows
the indicator. (Frontend-only dirty tracking would drift from agent-driven
edits via future control wiring.)

### 4. Prompts (frontend)

- `s` → quick save; `S` → name prompt (text input overlay; Enter saves to
  library via `slug`-checked name, Esc cancels; empty slug rejected inline).
- Exit with `dirty == true` (Esc to menu) → prompt "Save / Discard / Cancel"
  (`s`/`d`/Esc), matching the TUI flow.
- One-shot "saved → <dir>" flash after a successful save.

### 5. Router payloads

`{ kind: "edit", dir? }`: with `dir`, call `load_bundle` on mount (this makes
the #163 library `e` binding real); without, `Compose (new)` semantics.

## Tests

- Rust round-trip test on the command helpers: build a composer with a few
  notes → `save_bundle` to a temp dir → `load_bundle` → snapshots equal
  (notes, grid, key) and `meta.json` deserializes as `RecordingMeta`.
- Legacy load test: bundle with `song.mid` only (no meta) loads with default
  grid, as in the TUI.

## Scope boundaries (do NOT)

- Do not add audio (#166) — committing a chord produces effects that stay
  silent for now.
- Do not implement record/play screens.
- Do not change the bundle schema — TUI interop is the point.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] Chord selector: `c 3 s Enter` in a C-major piece places the same
      pitches as the TUI does for degree 3 seventh
- [ ] A bundle saved in Tauri opens in the TUI editor, and vice versa
- [ ] Dirty exit prompts; clean exit doesn't; `?` shows generated help
- [ ] PR opened against `main` from `claude/tauri-edit-overlays`, `Closes #165`
