# M24-A — Play: go to bar / seek command, 4-bar jumps, desktop seek bar

> Milestone: M24 — Play↔Edit polish · Issue: #337 · Suggested tier: sonnet
> Branch: `claude/m24-play-goto-bar-seek`

## Goal

Make skipping around Play fast, and give agents and the Edit → Play return
(M24-B) a way to say *which bar* to land on.

## Context

- `←`/`→` already step one bar in both frontends (`step_bar`). They pause, clamp
  to the song, and do nothing while a practice loop runs.
- **The two play screens number bars differently.**
  - The **TUI** play bar map is origin-shifted by the count-in, so its bar *n*
    is the composer's bar *n*.
  - The **desktop** `PlaySession::bar_map` counts whole pre-roll bars (the shift
    is rounded up to a bar) and ignores the grid origin, so its `bar` is **not**
    the composer's.
  - M23-A already hands over song µs for this reason.

## What to do

1. **Host commands** (`crates/control/src/host.rs`). Add them to the catalog,
   names and help; the parity tests must stay green.
   - `PlayGotoBar { bar: u64 }`: pause, then jump to the start of **editor**
     bar `bar` (0 = the piece's first bar; the count-in is not a bar), clamped to
     the song.
   - `PlaySeekUs { us: u64 }`: pause, then jump to the start of the bar that
     contains **song** time `us`.
   - Both are no-ops while a loop runs and return play status.
2. **TUI** (`play.rs`).
   - Add `goto_bar(bar) -> bool` and `seek_song_us(us) -> bool`;
     `step_bar` is reimplemented on top of `goto_bar`.
   - In `app.rs`, dispatch both host commands; `PgUp`/`PgDn` = `step_bar(∓4)`.
3. **Desktop backend** (`src-tauri/src/play.rs`).
   - Store the grid origin, then add
     `song_bar_map() = BarMap::new(bar_starts_us, bar_us).with_origin(origin + shift)`
     and build `goto_bar` / `seek_song_us` on it.
   - Add Tauri commands `play_goto_bar` / `play_seek_us`, registered and wired
     in `control.rs`.
4. **Desktop UI.**
   - `loopKeyCommand` gains `shift`: `Shift`+`←`/`→` and `PgUp`/`PgDn` seek ±4 bars.
   - Add a clickable seek strip along the bottom of `HighwayHeader`. A click
     calls `play_seek_us(seekBarSongUs(fraction, duration_us, shift_us))`; the
     fill shows `time_us / duration_us`.
   - Bridge: `playGotoBar`, `playSeekUs`.
5. **Docs.** Update `docs/AGENT-CONTROL.md` (commands) and
   `docs/TAURI-CONTROLS.md` (keys and seek bar).

## Tests

- **Host:** both commands parse from params, and each is listed in help.
- **TUI:**
  - `goto_bar(2)` lands at `SHIFT + 2·BAR`, paused, and clamps far bars;
  - `seek_song_us(1.3·BAR)` lands at `SHIFT + BAR`, and `seek_song_us(0)` at
    `SHIFT`;
  - both are refused while looping.
- **Desktop:**
  - a 4 s count-in session puts editor bar 0 at 4 s and bar 1 at 6 s;
  - `seek_song_us(2.5 s)` lands at 6 s;
  - a non-zero grid origin is honoured;
  - both are refused while looping.
- **Vitest:** big-skip key mapping and `seekBarSongUs` (count-in clamps to 0,
  fraction clamped to 0–1).

## Scope boundaries (do NOT)

- Don't change `←`/`→` semantics, or what Play's `bar` field reports.
- Seeks always pause. There is no seek-while-playing variant.
- No drag-scrubbing on the seek bar; a click is enough.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green; `npm test` and `tsc --noEmit` green
- [ ] PR opened against `main` from the branch above, `Closes #337`
