# M23-A — Pause play and jump into Edit at the current bar

> Milestone: M23 — Play↔Edit round-trip · Issue: #333 · Suggested tier: sonnet
> Branch: `claude/m23-play-to-edit-at-bar`

## Goal

While playing a piece you often spot a mis-edit. Today the only way to fix it is
to leave Play, reopen the bundle in Edit, and scroll back to the bar. Add a
single key, **`e`** on the Play screen of both frontends, that stops playback,
opens the **same bundle** in Edit, and puts the cursor on the **start of the bar**
the playhead was in.

## Context

- **No route Play → Edit exists** in either frontend. Both open Edit fresh from a
  bundle dir.
- **There is no absolute bar/time cursor move in core.** `Action` navigation
  (`crates/core/src/action.rs`) has `CursorBarLeft/Right` (± `steps_per_bar`),
  `CursorToStart/End`, and `SetCursor { pitch, step }`. The composer's tempo-map
  helpers are private: `steps_per_bar`, `bar_map`, `bar_at_us`
  (`crates/core/src/composer.rs`). The cursor step for bar *N* is
  `N × steps_per_bar()`, the same as `pos_us_of_step` and `CursorBarRight`
  assume, both with and without a tempo map.
- **Play-clock µs ≠ song µs.** Both play screens shift the song by `shift_us`
  (pre-roll + lead). So `song_us = play_us.saturating_sub(shift_us)`; the pre-roll
  clamps to 0.
  - **TUI:** `PlayScreen::now_us()` (private) and `shift_us` (`crates/tui/src/play.rs`).
  - **Desktop:** `PlayStateEvent.time_us` (`playState()` in `HighwayScreen.tsx`)
    and `shiftUs` (set from `PlayInfo.shift_us` in `startLive`).
  - The desktop play `bar` index includes whole pre-roll bars, so it is **not**
    the composer's bar index. That is why this spec hands time over, not bar
    numbers.
- **TUI tempo-map bug:** `App::open_edit_from_midi` (`crates/tui/src/app.rs`)
  never calls `composer.set_bar_starts(meta.bar_starts)`. The TUI save path
  (`edit.rs`, building `SongMeta`) writes `composer.bar_starts()`, which is empty
  in that case. So opening a tempo-mapped (M16) piece in the TUI editor and
  saving it **wipes its tempo map**, and its bars disagree with Play's. The
  desktop app loads the map correctly (`state.rs`). This feature lands you in
  the editor on a specific bar, so the bars must agree; fix it here.

## What to do

### 1. core: `Action::CursorToBarAt`

```rust
// crates/core/src/action.rs — navigation group, after SetCursor
/// Jump the cursor to the downbeat of the bar containing song time `us`
/// (tempo-map aware). Pitch unchanged. Used by Play → Edit.
CursorToBarAt { us: u64 },
```

- `name()` → `"cursor_to_bar_at"`. Add it to `action_names()`, and to
  `action_help()` with param `us: u64` and the description above. The parity
  tests must stay green.
- `Composer::apply`: `self.cursor.step = self.bar_at_us(us) * self.steps_per_bar()`.
  Produces no effects and is not an undoable edit (same as the other cursor moves).
- `us` before the grid origin → bar 0 → step 0.
- Desktop: add `"cursor_to_bar_at"` to the `ActionName` union
  (`tauri-app/src/ipc/types.ts`).

### 2. TUI

- **Fix the tempo map on open:** in `open_edit_from_midi`, load
  `meta.bar_starts` from the bundle's `meta.json` and apply it to the composer
  before the screen is shown. Reuse the existing meta-loading helpers (or add a
  `load_meta_bar_starts` next to `load_meta_grid_key`), plus an
  `EditScreen::set_bar_starts` pass-through if one doesn't exist.
- **Remember the bundle:** `PlayScreen` stores the `midi_path` it was loaded from
  (`Option<PathBuf>`), set where `load_play_screen` builds it.
- `pub fn PlayScreen::song_us(&self) -> u64`: `now_us().saturating_sub(shift_us)`.
- **Key `e`** on the running Play screen. It does nothing on the end-of-take
  summary and nothing when the screen has no `midi_path`. It:
  1. records `us = play.song_us()`,
  2. calls `play.leave()` (stops audio, like the existing exits),
  3. calls `open_edit_from_midi(path)`,
  4. if the screen is now `Screen::Edit`, dispatches `Action::CursorToBarAt { us }`.

  If the open fails, the status line explains why, as `open_edit_from_midi`
  already does.
- Add `e edit here` to the Play screen's key-hint line and to the help overlay
  if Play has one.

### 3. Desktop

- Route: `{ kind: "edit"; dir?: string; armed?: boolean; atUs?: number }`
  (`shell/screens.ts`). Pass `atUs` through `App.tsx` as an `EditScreen` prop.
- `EditScreen` `onMount`: after `loadBundle(props.dir)` resolves and the
  snapshot is applied, if `props.atUs !== undefined`, call
  `runAction("cursor_to_bar_at", { us: Math.max(0, Math.round(props.atUs)) })`
  and apply the returned snapshot. The canvas's normal follow-the-cursor
  behaviour then brings the bar into view.
- `HighwayScreen`: when live (`dir` defined) and a take is running (not before
  start, not on the summary), **`e`/`E`**
  `navigate({ kind: "edit", dir, atUs: (playState()?.time_us ?? 0) - shiftUs })`.
  The existing `onCleanup` already calls `playFinish()`, which tears the session
  down, so no explicit pause is needed.
- Add a small **"✎ Edit here"** button beside the transport's pause control,
  with the same handler and the same visibility rules.
- Update `docs/TAURI-CONTROLS.md` (Play keys) with `e`.

## Tests

- **core (`composer.rs`):**
  - uniform 4/4 grid, subdivision 4 (`spb = 16`), origin 0: a `us` inside bar 3
    → `cursor.step == 48`, pitch unchanged;
  - `us == 0` → step 0;
  - `us` exactly on bar 2's downbeat → step 32;
  - with a non-zero `grid.origin_us` and `us < origin` → step 0;
  - with `set_bar_starts` (uneven bars, e.g. `[0, 2_000_000, 3_500_000, 6_000_000]`):
    `us = 3_600_000` → bar 2 → step `2 × spb`.
- **core (`action.rs`):** `action_from_name("cursor_to_bar_at", {"us": 5})`
  round-trips, and a missing `us` errors (pattern: the `set_cursor` tests).
- **TUI:**
  - unit test that `open_edit_from_midi` on a bundle whose `meta.json` has
    `bar_starts` produces a composer with those `bar_starts`. Use a temp dir
    fixture, following existing `app.rs` tests;
  - `PlayScreen::song_us` subtracts `shift_us` and saturates at 0.

## Scope boundaries (do NOT)

- Do not add a `HostCommand` for this. Agents already have `load_bundle` plus
  `run_action cursor_to_bar_at`.
- Do not carry play state (practice loop, speed, hand, mixer) into Edit, and do
  not add an Edit → Play "resume at cursor" (possible follow-up M23-B).
- Do not change how either Play screen computes its own `bar` index.
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] desktop `npm run build` (tsc) clean
- [ ] PR opened against `main` from the branch above, `Closes #333`
