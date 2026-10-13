# M24-C — Slow down playback in Edit (`!` / `@`)

> Milestone: M24 — Play↔Edit polish · Issue: #339 · Suggested tier: sonnet
> Branch: `claude/m24-edit-playback-speed`

## Goal

Let the editor's transport run slower than the piece's own tempo, so a fast
passage can be checked note by note while editing. Both frontends (TUI and
desktop) get the same keys, the same speed steps as Play, a status badge, and
Play's audio rule: off 1× the backing recording is muted and the synth carries
the piece.

## Context

- Core already has `Action::SetPlaybackRate { rate_permille }`
  (`crates/core/src/action.rs`), `Composer::playback_rate()` /
  `set_playback_rate` (clamped 0.25–2.0), `Composer::advance` scaling `dt` by
  the rate, and `ComposerSnapshot.playback_rate`. No key dispatched it yet.
- Play steps its speed with `-` / `=` through `[500, 625, 750, 875, 1000]`
  permille, never above 1× (`crates/tui/src/play.rs::nudge_rate`, desktop
  `HighwayScreen.tsx` `RATE_STEPS`), and mutes an off-tempo backing
  (`PlaySession::backing_muted` in `tauri-app/src-tauri/src/play.rs`).
- TUI edit backing sync: `EditScreen::poll_backing` / `tick_backing`
  (`crates/tui/src/edit.rs`). Desktop edit backing sync: `audio::sync_backing`,
  called from `run_action` and the tick thread in `tauri-app/src-tauri/src/lib.rs`
  (which previously *resampled* the backing via `set_backing_speed`).
- Desktop playhead servo (`updateClock`) and video backdrop servo
  (`driveVideo`) in `tauri-app/src/screens/edit/EditScreen.tsx` assumed 1×.

## What to do

### Core — shared stepping helper

```rust
// crates/core/src/speed.rs (re-exported from lib.rs)
pub const RATE_STEPS: [u16; 5] = [500, 625, 750, 875, 1000];
pub const RATE_UNITY: u16 = 1000;
/// One step slower/faster; an off-step value snaps to the next step in that
/// direction; past either end it stays put.
pub fn nudge_rate_permille(current: u16, faster: bool) -> u16;
/// `Composer::playback_rate()` (f64) → permille, rounded.
pub fn rate_to_permille(rate: f64) -> u16;
```

TUI Play's `nudge_rate` reuses the helper (behaviour unchanged).

### Keys

- `!` = slower, `@` = faster, dispatching `SetPlaybackRate` with
  `nudge_rate_permille(current, faster)`. Matched on the **produced character**
  (`KeyCode::Char('!')`, `e.key === "!"`), so `!` is Shift+1 on any layout and
  `@` its own key on JIS. Not active in chord mode. Neither key is bound in the
  edit keymaps, and the mock keyboard only takes the digit row, so recording
  does not swallow them.
- A speed change is not an edit: it never marks the piece dirty.
- The rate is kept for the rest of the edit session (stop / replay keeps it);
  it is not saved to the bundle.

### Status + help

- TUI: a badge from `play::speed_badge` (e.g. `0.875×`) right after the
  PLAYING badge, only when not 1×. Help overlay lists `!` / `@`.
- Desktop: a `StatusBar` chip with the same label, only when not 1×; the hint
  strip and `HelpOverlay` list `!` / `@`. `docs/TAURI-CONTROLS.md` gets a row.

### Backing muted off 1×

- The backing "plays" only while the transport is playing **and** the rate is
  exactly 1× (`rate_to_permille(rate) == RATE_UNITY`).
- TUI: `poll_backing` / `tick_backing` use that as their playing signal — a
  step off 1× issues `Pause`; the step back issues `PlayAt(target)` at the
  current playhead (re-sync); a slowed play never starts the stream.
- Desktop: `audio::backing_audible(playing, rate)` feeds `sync_backing`'s
  `playing` in both `run_action` and the tick thread. `TransportSnapshot`
  keeps `playing` (backing audible, for `sync_backing`'s diff) and a new
  `transport_playing` (for the tick thread's play/stop snapshot emit).

### Desktop playhead + video servos

- `ComposerSnapshot` (TS) gains `playback_rate: number`; `ActionName` gains
  `"set_playback_rate"`.
- `updateClock` centres on the snapshot's rate: reset to `base` (not 1), servo
  `base + error/1e6` clamped to `[CLOCK_RATE_MIN·base, CLOCK_RATE_MAX·base]`.
  The lightweight `playhead` push uses the last snapshot's rate.
- `driveVideo` servos the backdrop around the base rate:
  `clamp(base + drift·0.8, max(0.25, base·0.5), 2)`, and its re-seek paths
  reset to `base` (not 1).
- Pure TS helpers in `screens/edit/speed.ts` (`nudgeRatePermille`,
  `rateToPermille`, `speedKey`, `speedLabel`) mirror core.

## Tests

- Core (`speed.rs`): slower from 1000 → 875 → 750 → 625 → 500 → 500; faster
  500 → 625, 875 → 1000, 1000 → 1000; off-step 800 slower → 750, faster → 875;
  250 faster → 500; 1500 slower → 1000; `rate_to_permille(0.875) == 875`.
- TUI (`edit.rs`): `!` sets the composer rate to 875 (and clamps at 500 after
  more presses), `@` steps back up and stops at 1000, never dirty; rate survives
  stop/replay; with a backing attached, a slowed play yields no `PlayAt`,
  returning to 1× yields `PlayAt(playhead)`, slowing mid-play yields `Pause`;
  the status line shows `0.875×` only off 1×.
- Desktop Rust (`audio.rs`): `backing_audible` true only for playing at 1.0.
- Desktop TS (`speed.test.ts`, vitest): the same stepping cases, key mapping,
  and `speedLabel(1) === ""`, `speedLabel(0.875) === "0.875×"`.

## Scope boundaries (do NOT)

- Do not change Play's speed behaviour (TUI Play only reuses the helper).
- Do not persist the rate to the bundle.
- No new third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `tsc --noEmit` and vitest green in `tauri-app`
- [ ] PR opened against `main` from the branch above, `Closes #339`
