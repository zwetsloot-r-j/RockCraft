# M10-E — Replace backing audio without dropping the video link

> Milestone: M10 — Split & Trim into Pieces · Issue: #221 · Suggested tier: sonnet
> Branch: `claude/m10-backing-swap-keep-video`
> Related: M9-E (#204, backing persistence), M9-G (#206, video persistence)

## Goal

Let a user swap the **backing audio** of the loaded piece while editing — for
example, replace the imported source-video audio with a cleaner studio track —
**without losing `meta.video`**, so the video keeps playing behind the grid. The
backing-audio link and the background-video link are independent piece
attributes; changing one must never clear the other.

## Context

- `RecordingMeta` carries `backing: Option<BackingTrack>` and
  `video: Option<BackgroundVideo>` as **separate** `#[serde(default)]` fields
  (`crates/core/src/song.rs`); M9-E persists backing to the loaded piece, M9-G
  persists video. The schema already supports independence — this issue
  guarantees the **edit/save behavior** matches it and exposes the swap UX.
- Attach/detach run through `HostCommand::AttachBacking { path }` /
  `DetachBacking` (`crates/control/src/host.rs`), implemented per frontend
  (`tauri-app/src-tauri/src/audio.rs`, `crates/tui/src/backing.rs`). The risk to
  close: an attach/save path that rebuilds `RecordingMeta` fresh (e.g.
  `RecordingMeta::new_midi_only(..)`) and forgets to carry `video` over, silently
  dropping the backdrop.

## What to do

1. **Guarantee preservation.** Audit the attach/detach + save paths in both
   frontends so that mutating `meta.backing` (attach, replace, or detach)
   **preserves** the existing `meta.video` (and `grid`/`key`/`origin`). Mutate the
   loaded meta in place rather than reconstructing it; if a fresh meta is built
   anywhere on this path, carry the other fields across.
2. **"Replace backing" affordance.** From the capture/edit screen, add/clarify a
   "replace backing audio" action that opens the existing backing picker for the
   **currently loaded** piece (reuse M9-E's entry point) and swaps the file.
   While the picker is open and after the swap, the **video backdrop stays
   visible/playing** (Tauri). Detach clears only the audio; the video remains.
3. **TUI parity.** The TUI has no video rendering, but its attach/detach/save
   path must equally **preserve `meta.video`** through a backing swap (round-trip,
   no loss). Note the render asymmetry in the PR.
4. Reflect the affordance in the help overlay / `docs/TAURI-CONTROLS.md`.

## Tests

- `core`/meta round-trip: start from a `RecordingMeta` with both `backing` and
  `video`; apply a backing **replace** (new `BackingTrack`) and a **detach**
  (`backing = None`); in both cases `video` is byte-identical afterwards. Add this
  alongside the existing `song.rs` round-trip tests.
- Frontend: a save/load round-trip where the loaded piece has a video, the
  backing is swapped, the piece is saved and reloaded → `meta.video` is unchanged
  and the (new) `meta.backing` is present, in both Tauri and TUI write paths.

## Scope boundaries (do NOT)

- Do **not** change the `BackingTrack` / `BackgroundVideo` schemas or the
  audio/video playback paths — this is edit/save-correctness + a UX entry point.
- Do **not** alter backing-alignment (`nudge_backing_offset`) or video-offset
  semantics.
- Do **not** add a swap path that isn't `AttachBacking`/`DetachBacking` — reuse
  the existing host commands.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Swapping or detaching backing audio on a piece with a video leaves
      `meta.video` intact and the backdrop visible/playing; verified by a
      save/load round-trip in both frontends
- [ ] PR opened against `main` from the branch above, `Closes #221`
