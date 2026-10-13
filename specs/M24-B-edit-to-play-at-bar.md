# M24-B — Edit → Play at the cursor's bar (`E`), saving in place

> Milestone: M24 — Play↔Edit polish · Issue: #338 · Suggested tier: sonnet
> Branch: `claude/m24-edit-to-play-at-bar` · Depends on M24-A (`play_seek_us`)

## Goal

This is the return half of M23-A. After fixing a mis-edit, **`E`** in the editor
takes you back to Play on the same bar, with your fix saved.

## Context

- Play → Edit is `e` (M23-A). Song µs is handed over because the desktop Play
  `bar` counts the count-in.
- M24-A adds `PlaySeekUs { us }`: pause and jump to the downbeat of the bar
  containing song time `us`.
- **TUI:** `EditScreen::save()` always wrote a **new** `take-<stamp>` bundle,
  and the editor didn't remember where the piece came from, so a fix never
  reached the bundle Play loads.
- **Desktop:** `s` already saves `in_place` over the backend's current bundle,
  and `is_same_file` guards the media copies.
- **TUI hazard:** `write_bundle` copied backing, video and background files into
  the bundle with `fs::copy`. Saving a bundle over itself copies each file onto
  itself, which truncates it to 0 bytes.

## What to do

1. **core:** make `Composer::cursor_us()` public (song µs, tempo-map aware).
2. **TUI `EditScreen`:**
   - Add `source_dir: Option<PathBuf>`, set by `open_edit_from_midi`, with a
     `source_dir()` getter.
   - `save(&mut self)` writes **in place** over `source_dir`. A piece without
     one becomes a new take, which is then remembered as its `source_dir`.
   - `save_to_library(&mut self, …)` also re-points `source_dir`.
   - Add `cursor_song_us()`.
   - Media copies go through `copy_into_bundle`, which skips the copy when the
     source and destination are the same file.
3. **TUI shell:** `E` in the editor calls `play_here()`:
   1. If the piece is dirty or has never been saved, save it (on failure, show
      the error in the status line and stay in Edit).
   2. Call `load_play_screen(source_dir/song.mid)` and `tuned`.
   3. Call `seek_song_us(cursor_song_us)`, which pauses.

   Add `E` to the hint and help lines.
4. **Desktop:**
   - The play route gains `atUs`. `HighwayScreen` calls `playSeekUs(atUs)` once,
     after `playLoad`, so the Start prompt / Space plays on from there.
   - `E` in the editor (not in background-layer mode) does
     `saveBundle({kind:"in_place"})`, then
     `navigate({kind:"play", dir: savedDir, atUs: cursorUsOf(snap)})`.
   - Add a help row and a `docs/TAURI-CONTROLS.md` row.

## Tests (TUI)

- An in-place `s` over an opened bundle saves to that directory, and the backing
  file's bytes survive.
- `E` opens Play on the same `song.mid`, paused, with `song_us` at the cursor
  bar's downbeat (tempo-mapped fixture). Play's `e` then lands back on the same
  editor step.
- `E` on a dirty piece rewrites the bundle's `song.mid` before opening Play.

## Scope boundaries (do NOT)

- No save prompt on `E`; the user chose save-in-place.
- No changes to the exit prompt, `S` naming, or split saves beyond the
  `source_dir` bookkeeping.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; `tsc --noEmit` and `npm test` green
- [ ] PR opened against `main` from the branch above, `Closes #338`
