# M7-tauri-N-edit-video-backdrop — Video backdrop for manual Synthesia transcription

> Milestone: M7 · Issue: #190 · Suggested tier: opus
> Branch: `claude/tauri-edit-video-backdrop`
> Depends on: M7-tauri-M (#189) (vertical edit orientation), #165 (edit overlays / save-load)

## Goal

Let the user attach a video (e.g. a Synthesia-style YouTube capture) and play it
**behind the edit grid**, with the visible frame synced to the on-screen
timeframe: as the editor scrolls through time, the backdrop shows the frame at
that song time. This makes it easy to hand-transcribe a piece by reading falling
notes off the video and placing them on the overlaid grid. Decode is done by the
webview via an HTML5 `<video>` element — **no backend ffmpeg frame extraction**.

## Context

- Edit screen after M7-tauri-M: vertical piano-roll, **time → y** (start at
  bottom, later up), **pitch → x** (low→high). `screens/edit/viewport.ts`
  exposes `yOf(us)` / `originUs` / `spanUs`, so the time at any on-screen line
  is known.
- The frontend is a webview: a local file can be loaded into a `<video>` via
  Tauri's asset protocol (`convertFileSrc` from `@tauri-apps/api/core`). The
  app already uses `@tauri-apps/plugin-dialog` for native file pickers
  (`RecordScreen.tsx`).
- Alignment: a video's `t = 0` rarely equals song `t = 0`. We need an
  adjustable **offset** (videoTime = songTime + offset) and a **scale** is *not*
  needed (assume 1:1 real-time playback; tempo mapping is out of scope).
- Persistence without touching `core`: the bundle schema is owned by `crates`
  and **must not change** here. Store the backdrop reference in a Tauri-only
  sidecar file inside the bundle dir.

## What to do

### 1. Attach / detach (frontend, `screens/edit/`)

- A new edit key (document it; suggest `V`) → native open dialog filtered to
  video extensions (`mp4 mkv webm mov m4v` — same family as the import picker;
  note mp4/h264/webm are the webview-playable ones). On select, set the
  backdrop video path in screen state.
- `V` again (or an overlay control) detaches/clears the backdrop.

### 2. Render the backdrop behind the grid

- Place a `<video>` element **behind** the `<canvas>` (lower z-index), sized to
  the grid region, with `src = convertFileSrc(path)`, `muted`, `playsInline`,
  and **paused** (we scrub it, not play it). Dim it (e.g. `opacity: 0.5` /
  a configurable dim) so the grid and notes stay legible on top. The canvas
  background fill from M7-tauri-M must be translucent or skipped where the video
  shows through.
- The canvas already draws the grid/notes; with M7-tauri-M leaving the grid
  background translucent-ready, the `<video>` shows through underneath.

### 3. Sync the frame to the on-screen timeframe

- Define the **reference line** as the cursor step time (or `playhead_us` while
  playing) — the same `anchorUs` the viewport scrolls around. On every snapshot
  / scroll / cursor move, set `video.currentTime = clamp((anchorUs + offsetUs) /
  1e6, 0, video.duration)`.
- While the composer `playing`, drive `video.currentTime` from the interpolated
  playhead each RAF (do not `video.play()` — keep it frame-accurate by seeking,
  which stays in lockstep with the playhead and avoids drift). If seeking-per-
  frame stutters, fall back to `play()` started at the offset and `pause()` on
  stop — note which you chose in the PR.

### 4. Alignment offset (frontend)

- Reuse the backing-nudge ergonomics: bind fine/coarse offset nudges so the
  user can line the video up to the grid (suggest the same `,`/`.` = ∓10 ms,
  `;`/`'` = ∓250 ms used for backing align in `edit.rs`, **but only while a
  backdrop is attached**, so they don't collide with backing-offset when no
  video is present — or pick distinct keys and document them). Show the current
  offset in the status bar / a small backdrop HUD.

### 5. Persist the reference (Tauri sidecar, optional)

- On save (hooking the existing #165 save flow), if a backdrop is attached,
  write a sidecar `transcription.json` into the bundle dir:
  `{ "video": "<abs or bundle-relative path>", "offset_us": <i64> }`.
- On load (`{ kind: "edit", dir }`), if `transcription.json` exists, re-attach
  the backdrop and restore the offset.
- This is a Tauri-app concern: read/write it in `src-tauri` (e.g. a
  `transcription_load(dir)` / `transcription_save(dir, dto)` command) **without**
  altering `core::song::RecordingMeta` or the `crates` schema.

## Tests

- `npx tsc --noEmit` passes.
- Rust: the sidecar read/write round-trips (`transcription_save` then
  `transcription_load` → equal DTO; missing file → `None`). Pure serde, temp dir.
- Manual (local): attach an mp4, confirm the frame under the cursor matches the
  song time, nudging the offset shifts the alignment, and scrolling/playing
  keeps frame and grid in sync.

## Scope boundaries (do NOT)

- Do not add backend ffmpeg frame extraction or a frame cache (webview decodes).
- Do not change `core` / `crates` / the bundle schema — backdrop state lives in
  the `transcription.json` sidecar only.
- Do not implement automatic note detection from the video (this is **manual**
  transcription; auto-extraction is M6's separate concern).
- Do not add tempo/scale mapping between video and song time (1:1 + offset only).
- Do not bundle any media or commit sample videos.

## Acceptance

- [ ] `cargo fmt --all --check` / clippy / `cargo test --workspace` clean
- [ ] `npx tsc --noEmit` passes; `scripts/check-no-media.sh` green
- [ ] `V` attaches a video; it renders dimmed behind the grid; the frame tracks
      the cursor/playhead time; offset nudges realign it
- [ ] Save then reopen the bundle re-attaches the backdrop at the saved offset
- [ ] PR opened against `main` from `claude/tauri-edit-video-backdrop`,
      `Closes #190`
