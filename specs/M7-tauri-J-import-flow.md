# M7-tauri-J-import-flow — Video file / URL import with progress screen

> Milestone: M7 · Issue: #170 · Suggested tier: sonnet
> Branch: `claude/tauri-import-flow`
> Depends on: M7-tauri-B-shell-menu (#162), M7-tauri-C-library (#163)

## Goal

Port the M6 import flow to Tauri: pick a local video (native dialog) or paste
a URL (when a fetch hook is configured), watch pipeline progress live, land
in the library on success.

## Context

- TUI reference: `crates/tui/src/import_screen.rs` — `VideoPicker`,
  `UrlInput`, `ImportingScreen` (worker thread + mpsc polling).
- Pipeline: `crates/import` — `import_video(ImportInput::{File, Url},
  on_progress)`, `Progress::{Fetching, Log(String), Extracting(f32),
  Writing, Done(PathBuf)}`, `fetch_command_configured()`,
  `ImportError` (incl. `NoFetchCommand`). Bundles land in `import-out/`
  (gitignored; `scripts/check-no-media.sh` guards the repo).
- `docs/IMPORT.md` — pipeline behaviour, sidecar, fixtures policy.
- Native dialog: `tauri-plugin-dialog` (added by #166; add it here if this
  lands first). Video extensions: same list as `VideoPicker`
  (`mp4 mkv avi mov webm flv wmv m4v`).

## What to do

### Backend `tauri-app/src-tauri/src/import.rs`

`rockcraft-tauri` gains a dependency on `rockcraft-import`.

```rust
#[tauri::command]
fn import_url_available() -> bool;            // fetch_command_configured()

#[tauri::command]
fn import_start(app: AppHandle, state: …, input: ImportInputDto)
    -> Result<(), String>;                    // ImportInputDto::{File(String), Url(String)}
```

- `import_start` rejects a second concurrent import (`Err("import already
  running")`), then spawns a `std::thread` running `import_video`, mapping
  each `Progress` to a webview event `"import_progress"`:
  `{ stage: "fetching" | "extracting" | "writing" | "done" | "failed",
     progress?: number, log?: string, bundle_dir?: string, error?: string }`
  (`Log` lines arrive under the current stage; pipeline `Err` → `failed`
  with `error.to_string()`).

### Frontend

- **Menu wiring (#162):** "Import from video file…" → native open dialog →
  on a chosen path, navigate `{ kind: "importing" }` and `import_start`.
  "Import from URL…" enabled iff `import_url_available()` (query at menu
  mount, replacing the #162 grey-out stub); selecting it shows the URL
  prompt.
- **`screens/import/UrlInput.tsx`:** centered modal text input — type,
  Backspace, Enter submits (non-empty), Esc cancels — TUI parity.
- **`screens/import/ImportingScreen.tsx`:** stage label, progress bar (only
  meaningful for `extracting`; indeterminate elsewhere), scrolling log pane
  (last ~200 lines, like the TUI), no Esc-cancel while running (the TUI
  cannot cancel either — say so in a footnote on screen). `done` → navigate
  to the library with the new bundle highlighted; `failed` → error panel
  with the log retained, Esc → menu.

## Tests

- Rust: `import_start` concurrency guard (second call errs while a fake
  long-running import holds the slot — factor the thread-spawn so a test can
  inject a stub pipeline fn).
- Progress→event mapping unit test (each `Progress` variant serializes to
  the documented payload).
- Frontend: `npx tsc --noEmit`. No media fixtures committed — extractor
  tests stay synthetic per `docs/IMPORT.md`.

## Scope boundaries (do NOT)

- Do not modify `crates/import` (pipeline behaviour is fixed; M6 owns it).
- Do not bundle yt-dlp/ffmpeg or any fetch tooling.
- Do not implement import cancellation (TUI parity).
- Do not commit any media or extracted MIDI outside `fixtures/`.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes; `scripts/check-no-media.sh` green
- [ ] URL item hidden/disabled without a fetch command; visible with
      `ROCKCRAFT_FETCH_CMD` set
- [ ] File import on a local sample (manual): stages + logs stream, ends in
      the library on the new bundle; a failing input shows the error panel
- [ ] PR opened against `main` from `claude/tauri-import-flow`, `Closes #170`
