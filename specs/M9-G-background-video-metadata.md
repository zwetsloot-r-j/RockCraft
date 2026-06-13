# M9-G — Background video saved to the piece (play/practice backdrop + imported source)

> Milestone: M9 — Tauri UX consolidation · Issue: #206 · Suggested tier: opus
> Branch: `claude/m9-background-video-metadata`
> Related: M7-tauri-N (#190, edit-grid video backdrop — merged), M3-H (#56, meta persistence)

## Goal

Let a user attach a **video to a piece** and play it as a background during play /
practice (and editing), with the reference **saved in the bundle's metadata** so
it travels with the piece. When a piece is **imported from a URL or a video file**,
save that original source video into the bundle and record it as the piece's
background video automatically.

## Context

- **Precedent (now merged):** M7-tauri-N added an edit-grid video *backdrop* in the
  Tauri webview — a local file loaded into an HTML5 `<video>` via Tauri's asset
  protocol (`convertFileSrc`), synced to song time with an adjustable offset, and
  stored **Tauri-side only (no `core` change)**. This spec promotes that to a
  **first-class, persisted** piece attribute and extends it to the play screen.
  Reuse N's `<video>` + offset-sync approach (`screens/edit/*`,
  `@tauri-apps/api/core` `convertFileSrc`).
- **Bundle metadata:** `crates/core/src/song.rs::RecordingMeta` already grew
  optional, `#[serde(default)]` fields (`backing`, `grid`, `key`, `origin`) the
  same backward-compatible way M3-H added `grid`/`key`. The bundle is a directory;
  `backing` shows the precedent for referencing a media file inside it.
- **Import pipeline:** `crates/import` + `tauri-app/src-tauri/src/import.rs`
  (`import_start`, file / URL). The downloaded/source video is available at import
  time; today it is not retained in the bundle.

## What to do

1. **Extend the bundle schema (in `core`, data only — no I/O).** Add an optional,
   backward-compatible background-video reference to `RecordingMeta`, e.g.:

   ```rust
   #[serde(default)]
   pub video: Option<BackgroundVideo>,   // None for pieces without one

   pub struct BackgroundVideo {
       pub file: String,        // bundle-relative filename, like `midi_file`/backing
       pub offset_us: i64,      // videoTime = songTime + offset (mirrors N's offset)
   }
   ```

   Mirror the existing optional-field + legacy-deserialize tests (the
   `minimal_legacy_json_deserializes` pattern). `core` stays pure — it only carries
   the reference, never decodes video.
2. **Persist & attach (Tauri).** Provide a "background video" entry point on the
   capture/edit screen (alongside backing — see M9-E) to pick a local video; copy
   it into the bundle and write `RecordingMeta.video`. Re-loading a piece restores
   it. The edit backdrop from N should read this persisted reference instead of its
   Tauri-only side store (migrate N's storage onto the bundle field).
3. **Play-screen backdrop.** On the play/practice screen
   (`tauri-app/src/screens/highway/*`), render the piece's background video behind
   the note highway, synced to song time with `offset_us`, reusing N's `<video>`
   sync logic. Hidden when the piece has no video.
4. **Import saves the source.** When importing from a URL or a video file
   (`import.rs` / `crates/import`), retain the original video in the resulting
   bundle and set `RecordingMeta.video` so imported pieces come with their backdrop
   already attached. Keep import working when no video is applicable (audio-only /
   failures) — the field stays `None`.
5. TUI: the TUI cannot render video; it must still **load and round-trip** the new
   `meta.video` field without loss (no rendering required). Note this asymmetry in
   the PR.

## Tests

- `core`: `RecordingMeta` with/without `video` round-trips; a legacy `meta.json`
  lacking `video` still deserializes (`video == None`).
- Import test (fixture-based where possible): an import that has a source video
  populates `meta.video` and places the file in the bundle; an import without one
  leaves `video == None`.
- Tauri: a piece with `meta.video` shows the backdrop on the play screen synced by
  `offset_us`; a piece without one shows none.

## Scope boundaries (do NOT)

- Do **not** add video decoding/ffmpeg frame extraction to any crate — decode is
  the webview `<video>` only (as in N). `core` carries the reference, nothing more.
- Do **not** break legacy bundles (field is optional, defaulted).
- Do **not** change the audio backing path (M9-E) or fold tempo mapping in
  (assume 1:1 real-time playback, like N).

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] A background video can be attached to a piece, persists in `meta.json`, and
      plays behind the highway on the play screen synced to song time
- [ ] Importing from URL / video file saves the source video into the bundle and
      sets it as the background video
- [ ] Legacy bundles without `meta.video` still load
- [ ] PR opened against `main` from the branch above, `Closes #206`
