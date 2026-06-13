# M9-D — Editable BPM in edit mode (new `core::Action`)

> Milestone: M9 — Tauri UX consolidation · Issue: #203 · Suggested tier: sonnet
> Branch: `claude/m9-bpm-editing`

## Goal

There is currently **no way to change a piece's tempo from edit mode**. `+`/`-`
adjust note **velocity** (`adjust_velocity`), not BPM — so the user's expectation
that `+`/`-` changes BPM is unmet, and no other binding does it either. Add a
first-class tempo edit: a `core::Action` to set/adjust BPM, wired into both
frontends and persisted with the piece.

## Context

- The tempo lives in the composer's `Grid` (`crates/core` — `Grid` holds
  tempo/time-sig/snap; persisted in `RecordingMeta.grid`, see M3-H). The
  `ComposerSnapshot` already exposes `bpm` (rendered by `StatusBar.tsx`).
- Composer actions are defined in `crates/core/src/action.rs` (`Action`,
  `action_from_name`, `action_help`, `ActionInfo`/`ParamInfo`, plus the parity
  tests that keep the catalog in lockstep). There is **no** BPM action today
  (confirmed: no `set_bpm`/`set_tempo` in `core`).
- Keymaps: `tauri-app/src/screens/edit/keymap.ts` and
  `crates/tui/src/edit.rs::key_to_action`. `+`/`=`/`-` are taken by
  `adjust_velocity`; `,`/`.`/`;`/`'` by backing nudge; `<`/`>` by subdivision.

## What to do

1. **Add the `core::Action`(s).** In `action.rs`, add tempo editing — both a
   coarse relative nudge and an absolute set:

   ```rust
   AdjustBpm { delta: i32 },   // clamp to a sane range, e.g. 20..=300
   SetBpm   { bpm: u32 },      // clamp to the same range
   ```

   Apply them to the composer `Grid` (single source of truth for tempo).
   Add `ActionInfo`/`ParamInfo` help entries and extend the **parity tests**
   (name/tag parity, `action_help` coverage, dispatch-from-help-params,
   uniqueness) exactly as existing actions do. Because actions are auto-wired,
   this also makes BPM editable over the agent-control socket for free.
2. **Bind keys in both frontends.** Choose unused keys (do not collide with the
   bindings above) — e.g. a tempo nudge on `Ctrl/Alt`-modified keys or a dedicated
   pair, plus a numeric **set-BPM prompt** (mirror the existing save-as text prompt
   in `EditScreen.tsx` / `edit.rs`) for absolute entry. Settle exact keys against
   `edit.rs` and document them in the status hint + `?` help overlay.
3. **Persistence.** Since tempo lives in `Grid` and `Grid` already serializes into
   `RecordingMeta.grid`, a changed BPM must be saved with the piece and reload at
   the edited tempo — verify the round-trip; no schema change expected.
4. Keep `+`/`-` as velocity (do not repurpose); the user's confusion is resolved
   by adding real BPM controls and documenting them, not by stealing those keys.

## Tests

- `core`: `AdjustBpm`/`SetBpm` change the grid tempo, clamp at the range bounds,
  and the snapshot `bpm` reflects it; the `action.rs` parity battery passes.
- Round-trip: a piece saved after a BPM change reloads at the new BPM (extends the
  M3-H meta-persist tests).
- Frontend: the bound key / prompt dispatches the action and `StatusBar` updates.

## Scope boundaries (do NOT)

- Do **not** change `adjust_velocity` or its `+`/`-` bindings.
- Do **not** add tempo-map / per-region tempo changes — single tempo per piece.
- Do **not** put tempo logic in a frontend; it lives in `core`'s `Grid`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] BPM is editable from edit mode (nudge + absolute set) in both Tauri and TUI,
      persists with the piece, and is reachable over the agent-control socket
- [ ] PR opened against `main` from the branch above, `Closes #203`
