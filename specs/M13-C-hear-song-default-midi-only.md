# M13-C — play: hear-the-song defaults on for MIDI-only pieces

> Milestone: M13 — Sheet Music Import · Issue: #247 · Suggested tier: sonnet
> Branch: `claude/m13-hear-song-default`

## Goal

A piece with no backing track should **make sound** when you play it. Today
"hear the song" defaults OFF everywhere, so a MIDI-only bundle opens silent
unless a live piano is connected and the user knows to press `m`. Make the
default derive from the bundle: no backing track → hear-the-song ON.

## Context

The synth side is already built and correct — this is a defaulting bug, not
missing capability. `SynthHandle` (`crates/audio`), the play-clock-driven
audition (M2-B, `specs/M2-audio-B-play-synth.md`) and the `m` toggle all work in
both frontends.

What's wrong is where the default comes from:

- **TUI** (`crates/tui/src/play.rs:125`) — `hear_song: false` at construction.
  The only thing that turns it on is a special case on the **import-completion**
  path (`crates/tui/src/app.rs:876`, added for #152: "without a live piano part,
  an import would otherwise be silent"). So an imported piece is audible exactly
  once; load that same bundle from the library afterwards and it is silent again.
- **Tauri** (`tauri-app/src-tauri/src/play.rs:313`) — `hear_song: false`, and
  `with_hear_song` is called **only from a test** (`play.rs:1025`). There is no
  import special case at all, so a MIDI-only bundle is silent in the desktop app
  no matter how it was loaded.

This matters now because M13-A (#245) adds score-file import, and score bundles
are **always** MIDI-only: the video path derives `backing.wav` with ffmpeg, but a
score has no audio to extract. Every imported score would land in the silent
case. The fix is not score-specific though, so it stands alone and does not
depend on #245.

`PlayInfo.has_backing` already exists in both frontends
(`tauri-app/src-tauri/src/play.rs:99`), so the rule is derivable where it's needed.

## What to do

**The rule:** at play-load, `hear_song` defaults to `meta.backing.is_none()`.
A piece with a real recording behind it doesn't need the synth doubling the
melody; a piece without one is otherwise silent. The `m` toggle keeps working in
both directions from whichever default applied.

- **TUI** — `load_play_screen` (`crates/tui/src/app.rs:1045`) already reads
  `meta.json` and branches on `meta.backing`; set the default in that same
  branch. Then **delete the import-only special case** at `app.rs:876` — it is
  superseded, and leaving both would make the import path differ from the
  library path for no reason.
- **Tauri** — apply the same rule in the `PlayLoad` path where `meta.json` is
  parsed and `with_backing` is (or isn't) applied.
- **No SolidJS change needed** — `HighwayScreen.tsx:152` already seeds its
  `hearSong` signal from `info.hear_song`, and `HighwayHeader` already renders
  the indicator. Verify this rather than assuming; the header must show the
  toggle lit on first paint for a MIDI-only piece.

## Tests

Both frontends, headless:

- Bundle **with** a backing track → `hear_song == false` after load.
- Bundle **without** a backing track → `hear_song == true` after load.
- The `m` toggle still flips in both directions from either default, and turning
  it off still silences sounding notes (existing `toggle_hear_song` behaviour
  must not regress).
- A MIDI-only bundle loaded from the **library** and the **same bundle loaded
  after an import** end up in the same state — this is the regression the
  deleted special case used to hide.
- Existing M2-B trigger-bookkeeping tests stay green.

## Scope boundaries (do NOT)

- Do not change the synth, the audition trigger logic, or the `m` keybinding.
- Do not add a user-facing preference for the default — the bundle decides.
- Do not touch the record or edit paths; this is play-load only.
- Do not make this depend on #245. It fixes a gap that exists today.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] Manual (local): a MIDI-only bundle opened from the library is audible
      immediately in both frontends, with the header indicator lit
- [ ] PR against `main` from `claude/m13-hear-song-default`, `Closes #247`
