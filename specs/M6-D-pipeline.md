# M6-D — import: pipeline orchestration (resolve input, run sidecar, write chart)

> Milestone: M6 — Video Import · Issue: #116 · Suggested tier: sonnet
> Branch: `claude/m6-pipeline`

## Goal

The Rust orchestration in `rockcraft-import` that turns an input into a chart:
**resolve the input** (local file, or a URL via a pluggable fetch hook) → **run
the M6-C sidecar** → **parse + write the bundle** (M6-A) into the gitignored
output dir. One headless entry point the menu (M6-E) calls.

## Context

- Crate `rockcraft-import` (from M6-A, #113). Still depends inward on
  `rockcraft-core` only for types; subprocess + env are std-only.
- Sidecar: `tools/synthesia-extract/extract.py` (M6-C, #115), invoked as a
  subprocess.
- Output paths + gitignore from M6-B (#114): `import_output_dir()` and an
  `import-cache/` for fetched media.
- **No downloader in the repo.** Downloading is delegated to a user-configured
  external command (the private wrapper, e.g. yt-dlp, lives in
  `scripts/local/`, gitignored). The committed code only knows a generic hook.

## What to do

```rust
pub enum ImportInput { File(std::path::PathBuf), Url(String) }

pub enum Progress { Fetching, Extracting(f32 /*0..1*/), Writing, Done(std::path::PathBuf) }

/// Resolve → extract → write. `on_progress` lets the TUI render status.
pub fn import_video(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<std::path::PathBuf, ImportError>;
```

1. **Resolve input.**
   - `File(p)`: use `p` directly (must exist; reject tracked-tree paths only if
     trivially detectable — not required).
   - `Url(u)`: invoke the **fetch hook** — the command in `$ROCKCRAFT_FETCH_CMD`
     (or `scripts/local/fetch.sh` if present), passed the URL and a target path
     inside `import-cache/`. The command is expected to drop a local video
     there; the orchestrator then continues with that file. If **no** fetch
     command is configured, return `ImportError::NoFetchCommand` with an
     actionable message ("set ROCKCRAFT_FETCH_CMD or add scripts/local/fetch.sh
     — see docs/IMPORT.md"). The repo never names YouTube or bundles yt-dlp.
2. **Run the sidecar.** Locate `tools/synthesia-extract` and run `extract.py`;
   surface a clear `ImportError::SidecarMissing` if its interpreter/deps aren't
   set up. Map sidecar progress (if any) to `Progress::Extracting`.
3. **Parse + write.** Read the sidecar's M6-A JSON, `chart_to_timeline` +
   `write_chart_bundle` into a fresh `import_output_dir()/<slug-stamp>/`. Return
   the bundle path; emit `Progress::Done`.

## Tests (headless)

- Parse+write: feed a recorded **synthetic** sidecar JSON (M6-A fixture) through
  the parse/write half (inject the sidecar step or run with a stub script that
  echoes the fixture) → a valid bundle under a temp output root.
- `Url` with no fetch command configured → `NoFetchCommand`.
- `Url` with a **stub** fetch command (a tiny script that copies a local
  synthetic file into the cache) → proceeds to extraction (stub sidecar) and
  writes a bundle. (Tests must not hit the network or name YouTube.)
- Missing sidecar → `SidecarMissing` with a helpful message.

## Scope boundaries (do NOT)

- No yt-dlp / YouTube-specific code anywhere — only the generic
  `$ROCKCRAFT_FETCH_CMD` / `scripts/local/fetch.sh` seam.
- No CV/audio logic (that's the sidecar); no TUI (that's M6-E).
- Never write a chart outside `import_output_dir()`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m6-pipeline`, `Closes #116`
