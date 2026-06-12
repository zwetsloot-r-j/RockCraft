# M7-tauri-K-demock-entry-points — Remove demo autoplay; wire real "last recording" entry points

> Milestone: M7 · Issue: #187 · Suggested tier: sonnet
> Branch: `claude/tauri-demock-entry-points`
> Depends on: #168 (play live), #169 (record live), #163 (library scanner)

## Goal

Stop the Tauri app from auto-playing the "Ember Lantern" mock on the Play and
Record screens. The live wiring already exists (#168/#169); what remains is to
**route the real bundles in** and **retire the demo fallbacks** so a normal
launch never shows fixture data. After this, every screen shows real state or a
clear empty state — never a canned song.

## Context

- `screens/menu/MenuScreen.tsx` — "Play last recording" routes
  `navigate({ kind: "play" })` with **no `dir`**, and "Edit last recording"
  routes `navigate({ kind: "edit" })` with **no `dir`** (identical to
  "Compose (new)"). Both currently land in demo / empty.
- `screens/highway/HighwayScreen.tsx` — `live = dir !== undefined`. When `dir`
  is absent it builds `new HighwayCanvas(canvasEl, cfgFusion, SONG)` and runs
  the mock clock. `SONG` comes from `screens/highway/song.ts`.
- `screens/record/RecordScreen.tsx` — `onMount` **always** constructs
  `new RecordCanvas(canvasEl, …, RSONG)` and calls `e.start()`, animating the
  `RSONG` fixture (`screens/record/song.ts`) regardless of session state.
- Backend: `src-tauri/src/library.rs` exposes `scan_library_inner(roots)` over
  `rockcraft_midi::bundle::list_library`; recordings land in
  `recordings/take-<unix_ts>/`. There is **no** "latest recording" helper yet.

## What to do

### 1. Backend: a "latest recording" command (`src-tauri/src/library.rs`)

```rust
#[tauri::command]
fn latest_recording() -> Option<String>;  // newest bundle dir, or None
```

- Scan the same default roots as `scan_library_inner`, pick the **newest**
  bundle by directory timestamp (the `take-<unix_ts>` name sorts
  chronologically; for library/imported bundles fall back to filesystem mtime).
- Return the absolute dir as a `String`, or `None` when there are no bundles.
- Factor the selection into a pure `fn newest(entries: &[(String, u64)]) -> Option<String>`
  (name+mtime pairs) so it is unit-testable without a filesystem.

### 2. Menu wiring (`MenuScreen.tsx`)

- "Play last recording": `await latestRecording()`; if `Some(dir)` →
  `navigate({ kind: "play", dir })`; if `None` → stay on the menu and show a
  brief inline notice ("No recordings yet — record or import one first").
- "Edit last recording": same, → `navigate({ kind: "edit", dir })` / notice.
  This makes the item genuinely distinct from "Compose (new)".
- Add `latestRecording` to `ipc/bridge.ts`.

### 3. Play screen: retire the demo fallback (`HighwayScreen.tsx`)

- Remove the `else` demo branch that runs `HighwayCanvas` on `SONG`. Opening
  Play **without** a `dir` is now an empty state: a centered "Nothing to play —
  open a bundle from the Library" with Esc → menu. (Reaching it requires a
  bundle, so this is just a guard.)
- Delete the `SONG` import from this screen.

### 4. Record screen: start idle, not animating (`RecordScreen.tsx`)

- Do **not** feed `RSONG` to `RecordCanvas`. Construct it with an empty take so
  the canvas shows the bare keyboard + empty staff until live notes arrive.
  (If `RecordCanvas`'s constructor requires a song, pass an empty
  `{ notes: [], … }`; do not animate it on mount.)
- Delete the `RSONG` import from this screen.

### 5. Retire the fixtures

- Move `screens/highway/song.ts` and `screens/record/song.ts` out of the
  shipped path. Either delete them, or relocate to `screens/<x>/demo.ts` and
  reference **only** from a dev-only `?demo` query-param path documented in the
  file header. Default launch must not import them. Remove now-dead exports
  flagged by `tsc`.

## Tests

- Rust: `newest()` picks the highest `take-<ts>` and breaks ties by mtime;
  empty input → `None`.
- `npx tsc --noEmit` passes with the fixtures removed (proves nothing on the
  default path still imports them).

## Scope boundaries (do NOT)

- Do not change `core`, `crates/midi`, or the bundle schema.
- Do not alter the live play/record logic from #168/#169 — only the entry
  routing and the demo fallbacks.
- Do not implement the controls cleanup (that is M7-tauri-L) or any edit-screen
  changes (M7-tauri-M/N).

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes
- [ ] Fresh launch → "Play last recording" with no bundles shows the notice;
      after recording one, it opens that take live (no Ember Lantern)
- [ ] "Edit last recording" opens the newest bundle, distinct from "Compose (new)"
- [ ] Record screen on entry is idle (no fixture animation); live notes draw
- [ ] PR opened against `main` from `claude/tauri-demock-entry-points`,
      `Closes #187`
