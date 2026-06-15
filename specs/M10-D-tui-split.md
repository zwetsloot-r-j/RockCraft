# M10-D — TUI split points + save parts (parity, no video render)

> Milestone: M10 — Split & Trim into Pieces · Issue: #220 · Suggested tier: sonnet
> Branch: `claude/m10-tui-split`
> Depends on: M10-B (#218, `SplitBundle`)
> Parallel to: M10-C (#219, the Tauri editor)

## Goal

Bring split/trim to the TUI editor so the two frontends stay in parity: keys to
drop/remove split markers at the playhead/cursor, list the resulting segments,
toggle each keep/discard, name a kept part, and save the kept parts via the same
`SplitBundle` host command. The TUI cannot render video, but the part bundles it
creates must round-trip their `backing`/`video` references without loss.

## Context

- The TUI editor is `crates/tui/src/edit.rs` (grid/cursor) with the screen state
  in `app.rs`; saving/library live in `library.rs` / `library_screen.rs`. Backing
  selection is `backing.rs` (M9-E). These are the screens to extend.
- `SplitBundle` (M10-B) is the shared write path; the TUI's `HostServices` impl
  already dispatches host commands — invoke `SplitBundle` the same way the editor
  invokes `SaveBundle`/`AttachBacking`.
- `core::segment::segments_from_splits` derives the consecutive segments from the
  marker set — reuse it directly (TUI depends on `core`).

## What to do

1. **Markers via keys.** Add keybindings to drop a split marker at the current
   cursor/playhead song-time, remove the nearest marker, and clear all. Track the
   marker set in the edit-screen state and render them in the grid ruler (e.g. a
   distinct column glyph / tick) so they're visible without video.
2. **Segment list + keep/discard + name.** Show the derived segments (from
   `segments_from_splits`) in a panel: index, time range, keep/discard flag
   (default keep), and a name (default `part-N`). Keys to toggle keep/discard and
   to rename the selected segment (reuse the TUI's existing text-prompt pattern).
3. **Save parts.** A key gathers the kept segments into
   `SegmentSpec { start_us, end_us, name }[]` and calls `SplitBundle`; report the
   created bundle count/paths in the status line and refresh the library. The
   source piece is left unchanged.
4. **No video rendering** — the TUI shows markers/segments textually only. But the
   created part bundles must still carry the correct `meta.backing`/`meta.video`
   (derived offsets) so opening them later (in Tauri) restores audio + backdrop;
   assert this in the round-trip test below.
5. Add the new keys to the TUI help overlay.

## Tests

- Segment derivation in the TUI maps marker set → `SegmentSpec`s identically to
  `core::segment` (same boundaries; discarded omitted).
- Round-trip: starting from a fixture bundle with `backing` + `video`, a
  `SplitBundle` invocation driven through the TUI write path produces part
  bundles whose `meta.json` has the derived `audio_start_us`/`offset_us` and
  whose media files are present — i.e. **no loss of the video/backing reference**
  even though the TUI never renders them.

## Scope boundaries (do NOT)

- Do **not** render or decode video in the TUI.
- Do **not** fork slicing/bundle-writing logic — reuse `core::segment` +
  `SplitBundle`.
- Do **not** modify or delete the source bundle.
- Backing-swap-keeps-video is M10-E.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] TUI can drop/remove markers, keep/discard + name segments, and save kept
      parts via `SplitBundle`; created bundles round-trip backing/video refs
- [ ] PR opened against `main` from the branch above, `Closes #220`
