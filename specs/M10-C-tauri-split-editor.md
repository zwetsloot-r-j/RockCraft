# M10-C — Tauri split-marker editor (keep/discard/name) with video backdrop

> Milestone: M10 — Split & Trim into Pieces · Issue: #219 · Suggested tier: opus
> Branch: `claude/m10-tauri-split-editor`
> Depends on: M10-B (#218, `SplitBundle`)
> Related: M9-A (#202, unified capture/edit screen), M9-G/M7-N (video backdrop)

## Goal

Give the user a way, on the unified capture/edit screen, to drop **split
markers** along the timeline — dividing the piece into consecutive segments —
flag each segment **keep** (with a name) or **discard**, and **save the kept
parts** as standalone pieces via `SplitBundle` (M10-B). The video backdrop
stays visible the whole time so the user can see where to cut.

## Context

- The capture/edit screen is the M9-A unification (`tauri-app/src/screens/edit/*`
  and its store). It already shows the note grid, transport/loop region (M9-C),
  BPM (M9-D), backing affordance (M9-E), and the persisted **video backdrop**
  (M9-G, built on M7-N's `<video>` + `convertFileSrc` + `offset_us` sync).
- Host commands reach Tauri through the `HostServices` impl in
  `tauri-app/src-tauri`; invoke `SplitBundle` the same way the screen already
  invokes `SaveBundle`/`AttachBacking`. Discover param shape via the live
  `query help` catalog if needed.
- Loop-region selection (M9-C) is the closest existing "range on the timeline"
  interaction to model the marker UX on.

## What to do

1. **Split markers.** Add an edit-screen affordance to drop a split marker at the
   current playhead/cursor song-time, remove the nearest marker, and clear all.
   Render markers as vertical lines across the grid (and along the transport/
   video scrubber). Markers divide the piece into consecutive segments
   `[0,m1), [m1,m2), …, [mN, end)` — mirror `core::segment::segments_from_splits`
   semantics (sorted, deduped).
2. **Per-segment keep/discard + name.** Show the segments as a labeled strip/list
   under the grid: each has a keep/discard toggle (default keep) and an editable
   name (default e.g. `part-1`, `part-2`, …). Discarded segments are visually
   dimmed. Keep at least one segment keepable; surface a clear empty/all-discarded
   state.
3. **Save parts.** A "Save parts" action gathers the **kept** segments into
   `SegmentSpec { start_us, end_us, name }[]` and calls `SplitBundle`. On
   success, show the created bundle paths/count (toast or library refresh) so the
   new pieces appear in the library. Saving does not modify the source piece.
4. **Video backdrop stays visible** throughout (the whole point — "trim by
   watching the video"). Reuse the M9-G backdrop; markers should be readable over
   it. No new video plumbing.
5. Keep keyboard + on-screen controls consistent with the screen's existing
   control conventions; add entries to the help overlay / `docs/TAURI-CONTROLS.md`.

## Tests

- A component/integration test (the repo's existing Tauri front-end test style):
  given a piece, dropping two markers yields three segments; toggling the middle
  to discard and invoking "Save parts" calls `SplitBundle` with exactly the two
  kept `SegmentSpec`s (correct `start_us`/`end_us`/names).
- `segments_from_splits`-parity check on the front-end segment derivation (same
  boundaries as core for the same marker set).
- Backend (`SplitBundle`) behavior is covered by M10-B; this issue asserts the
  wiring/derivation, not re-testing the write path.

## Scope boundaries (do NOT)

- Do **not** reimplement slicing or bundle writing in TypeScript — derive
  segments and call `SplitBundle`; all media/MIDI writing is M10-A/B.
- Do **not** add a bespoke IPC command — go through the `SplitBundle`
  `HostCommand`.
- Do **not** change the video backdrop or backing plumbing (M9-E/G); reuse them.
- TUI parity is M10-D; backing-swap-keeps-video is M10-E.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; front-end checks (lint/test) green
- [ ] Markers divide the piece; segments can be kept/discarded/named; "Save parts"
      creates the kept bundles via `SplitBundle` with the video backdrop visible
- [ ] PR opened against `main` from the branch above, `Closes #219`
