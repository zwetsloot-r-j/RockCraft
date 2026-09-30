# M18-C — TUI practice hand

> Milestone: M18 · Issue: #298 · Suggested tier: opus
> Branch: `claude/tui-practice-hand`

## Goal

Let the TUI player practise one hand: the app plays the other hand, wait mode
waits only for the practised hand, and the practice loop's demo plays just the
practised hand — as the desktop app does.

## Context

- TUI play screen: `crates/tui/src/play.rs`. It already knows each note's hand
  (`hands`, from `with_hands(split, overrides)`: the per-note override, else the
  split line) and colours the highway by it.
- Desktop reference: `tauri-app/src-tauri/src/play.rs` — `practice:
  Option<Hand>`, `set_practice`, `spans_for`, `rebuild_wait_gate`, `autoplays`
  (outside a loop, and per loop phase), and `enter_phase`'s hand snapshot.
  Keys: `h` cycles both → right → left (`HighwayScreen.tsx`). The highway greys
  out the other hand (`HighwayCanvas.ts`, `isAutoplayed`).
- Host command `play_set_practice { hand: "left" | "right" | null }` exists;
  the TUI answers `Unsupported`.
- The TUI practice loop (M17-B): `autoplays`, `enter_phase`, `RunningLoop`.
- Related: `specs/M14-E-per-note-hand-override.md`, `specs/M17-A-play-practice-loop.md`.

## Behaviour

`practice: Option<Hand>` on `PlayScreen`; `None` = both hands (today).

- **Keys:** `h` cycles both → right → left → both.
- **Wait gate:** built from the practised hand's notes only (both hands when
  `None`). Changing the hand rebuilds it at the playhead (outside a loop).
- **Autoplay (outside a loop):** a note sounds on the song bus when "hear the
  song" is on (every note), or when it belongs to the *other* hand while one
  hand is practised — whatever "hear the song" says, so the accompaniment is
  always there.
- **In the practice loop:** the hand is snapshotted at each phase boundary, so a
  change mid-loop takes effect at the next phase (as in M17-A):
  - count-in: nothing autoplays;
  - demo: the practised hand only (both when `None`); the other hand is silent;
  - your turn: the other hand autoplays; "hear the song" adds the practised
    hand too; the loop's gate holds only the practised hand's notes.
- **Highway:** with one hand practised, the other hand's notes are drawn faded
  (reuse the loop's fade toward the background).
- **Status line:** `[h] both` / `[h] right` / `[h] left` among the hints, lit
  when a hand is chosen.
- `play_set_practice` sets it and returns `{ "practice": "left"|"right"|"both" }`;
  off the play screen it fails cleanly.
- `restart` keeps the choice.

## Tests

With a two-hand chart (left notes below the split, right above, plus one
per-note override):
- Right-hand practice: the gate waits for right notes only; left notes autoplay
  with "hear the song" off; the override note follows its override.
- Both: nothing autoplays with "hear the song" off (unchanged behaviour).
- Changing the hand mid-song rebuilds the gate at the playhead (a frozen left
  step un-freezes when switching to right).
- Loop, right hand: the demo sounds right notes only; your turn sounds left
  notes only (hear-song off) and waits for right notes only; switching to left
  during the demo applies from the next count-in.
- Shell: `h` cycles; `play_set_practice` works on the play screen and fails off
  it.

## Scope boundaries (do NOT)

- No scoring (that's M18-D, which will score only the practised hand).
- No editing of the split line or per-note hands here (they come from the
  bundle, as today).
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #298`
