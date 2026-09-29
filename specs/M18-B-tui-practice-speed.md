# M18-B — TUI practice speed

> Milestone: M18 · Issue: #297 · Suggested tier: sonnet
> Branch: `claude/tui-practice-speed`

## Goal

Let the TUI play screen slow a take down (and back up), like the desktop app:
the highway, wait gate, count-in and practice loop all run slower together, so
a hard passage can be learnt at half speed.

## Context

- TUI play screen: `crates/tui/src/play.rs` (`PlayScreen::advance`, `tick`).
- Desktop reference: `tauri-app/src-tauri/src/play.rs` — `rate_permille`,
  `PLAY_RATE_UNITY` (1000), `PLAY_RATE_MIN` (250), `PLAY_RATE_MAX` (2000),
  `set_rate`, and the scaling at the top of `advance`. The keys and steps are in
  `HighwayScreen.tsx` (`RATE_STEPS = [500, 625, 750, 875, 1000]`, `-` / `=`).
- The host command `play_set_rate { rate_permille: u16 }` exists
  (`crates/control/src/host.rs`); the TUI answers `Unsupported`.
- The practice loop (M17-B, `specs/M17-B-tui-practice-loop.md`) runs off the
  same clock, so it slows down for free.

## Behaviour

- Speed is scaled into the time entering the session, never into the chart: in
  `advance`, `dt_us = dt_us * rate_permille / 1000`. The clock, wait gate,
  loop phases and count-in clicks all stretch together.
- Keys on the play screen: `-` one step slower, `=` one step faster, through
  `500, 625, 750, 875, 1000` (0.5×–1×). Keys never go past 1×; from a value off
  the steps (set over the socket) they move to the nearest step in that
  direction.
- `play_set_rate { rate_permille }` sets any value, clamped to 250–2000, and
  returns `{ "rate_permille": <applied> }`. Off the play screen it fails cleanly
  (like `play_toggle_pause`).
- **Backing:** it can't follow a changed speed, so it is muted (level 0) while
  the rate is not 1×, as in the desktop app. A muted backing keeps running at
  full speed and drifts ahead of the slowed clock, so on returning to 1× it is
  restarted at the clock's position (drop the handle; `tick_backing` re-arms it,
  the same path a loop jump uses) and brought back to the mixer's level.
- **Hear the song** and the loop demo play at the slowed clock (the synth follows
  the clock, so notes simply come later).
- Status line: a `0.75×` badge next to the clock when not 1×, plus `[-/=] speed`
  among the hints.
- `restart` keeps the chosen speed.

## Tests

- `advance(1_000_000)` at 500‰ moves the clock by 500 000 µs; at 1000‰ by
  1 000 000.
- Steps: from 1000, `-` ×5 → 500 (stays); `=` from 500 → 625; from 700 (set by
  socket) `-` → 625, `=` → 750; `=` at 1000 stays.
- `set_rate` clamps 100 → 250 and 5000 → 2000.
- A running loop at 500‰ reaches its loop start after two count-in bars'
  worth of wall time; clicks still fire once per beat.
- Backing gain is 0 at 750‰ and back to the mixer level at 1000‰, and the
  return to 1000‰ re-arms the backing at the clock position (test through the
  gain / target the screen would apply; no audio device needed).
- Shell: `-` / `=` reach the screen; `play_set_rate` works on the play screen
  and fails off it.

## Scope boundaries (do NOT)

- No time-stretching of the backing audio (it mutes instead).
- No speed in the editor transport (it has its own).
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #297`
