# M19-B — Find similar passages & replay an edit diff there

> Milestone: M19 — Edit replication · Issue: #317 · Suggested tier: opus
> Branch: `claude/m19-edit-replication`
> **Depends on M19-A (#316)** — revision IDs and `EditDiff` must be on `main`.

## Goal

Let an agent say: *"take the edits between rev 41 and now, find every other
passage that looks like the original, and apply the same edits there"* — e.g.
fix a voicing once and propagate it to every repeat of the phrase. Two pieces:
a read-only **`FindSimilar` query** (with a dry-run per match) and one pure
**`ApplyEditDiff` action** that applies the diff at chosen targets as a single
undo step.

## Context

- M19-A: `History` revisions, `Composer::edit_diff`, `EditDiff`/`DiffBounds`
  (`crates/core/src/edit_diff.rs`), `QueryKind::History`/`EditDiff`.
- Actions: `crates/core/src/action.rs` (+ catalog/parity tests); composer
  dispatch `Composer::apply`. `SetBarStarts { bars_us: Vec<u64> }` is the
  precedent for a `Vec` field on an `Action`.
- Pitch range is A0–C8 (21..=108); `InsertRun` is the precedent for "a note
  already occupying the target cell is replaced".

## Terms

- **Source**: the `EditDiff` between `from_rev` and `to_rev` (default current).
- **Pattern**: the notes of the `from_rev` timeline inside the **window** —
  by default the diff's `DiffBounds` (onset in `[us_lo, us_hi)`, pitch in
  `[pitch_lo, pitch_hi]`); the caller may pass an explicit window instead
  (e.g. the whole bar, to include context the edit didn't touch).
- **Target**: an offset `{ dt_us: i64, transpose: i8 }` at which the pattern
  occurs in the **current** timeline.
- **Note match** under a target: a current note with
  `pitch == p.pitch + transpose` and `|start − (p.start + dt)| <= tolerance_us`
  (nearest wins; each current note matches at most one pattern note).

## What to do

### 1. Pure search & dry-run (core)

New module `crates/core/src/replicate.rs`:

```rust
pub struct FindParams {
    pub window: Option<Window>,     // default: diff bounds
    pub allow_transpose: bool,      // default false
    pub tolerance_us: u64,          // default 30_000
    pub min_score_permille: u16,    // default 1000 (exact layout)
}
pub struct Match {
    pub dt_us: i64,
    pub transpose: i8,
    pub score_permille: u16,        // matched pattern notes / pattern size
    pub extra_notes: u32,           // current notes in the shifted window not matched
    pub window: Window,             // the shifted window in current song time
    pub applicable: bool,           // dry-run of ApplyEditDiff at this target succeeds
    pub conflicts: Vec<String>,     // why not, when !applicable
}
pub fn find_similar(from: &Timeline, current: &Timeline, diff: &EditDiff, p: &FindParams)
    -> Result<Vec<Match>, ReplicateError>;
```

- Candidates: anchor on the pattern's first note; every current note with the
  right pitch (any pitch when `allow_transpose`) yields a `(dt, transpose)`.
- Exclude the **source location itself** (`dt == 0 && transpose == 0`) and any
  candidate whose window overlaps the source window.
- Keep matches with `score >= min_score_permille`; resolve overlapping
  candidates greedily by score then earliest time. Return sorted by `dt_us`.
- Errors: `EmptyDiff`, `EmptyPattern` (window holds no `from` notes — tell the
  caller to widen the window), `DiffTooWide` (diff bounds span more than
  **8 bars** — ripple/tail edits like `nudge_tail`, `insert_bar` are not local
  and can't be replicated).

```rust
pub fn apply_diff_at(current: &mut Timeline, diff: &EditDiff, target: Target, tolerance_us: u64)
    -> Result<(), Vec<String>>;  // all-or-nothing per target
```

Per target, resolve everything first, then mutate only if nothing conflicts:
- **removed** `r`: the note matching `r` shifted → delete. Missing → conflict.
- **changed** `before→after`: the note matching `before` shifted → apply the
  **deltas** (`after − before` for start, dur, velocity, pitch; set `hand` to
  `after.hand`) so the target's own small timing deviations are preserved.
  Missing → conflict.
- **added** `a`: insert `a` shifted; a note at the same pitch whose onset is
  within `tolerance_us` is replaced.
- Any resulting pitch outside 21..=108, negative time, or zero duration →
  conflict.

### 2. Control query

`QueryKind::FindSimilar { from_rev, to_rev: Option<u64>, #[serde(flatten)] params }`
(serde defaults as listed) → `Response::Matches { from_rev, to_rev, matches }`.
`applicable`/`conflicts` come from running `apply_diff_at` on a **clone**.

### 3. The action

```rust
/// Replay the note edits between two revisions at each target offset, as ONE
/// undo step. Targets that would conflict are skipped (all-or-nothing per
/// target); run `FindSimilar` first to see which apply.
ApplyEditDiff { from_rev: u64, to_rev: u64, targets: Vec<EditTarget>, tolerance_us: u64 },
pub struct EditTarget { pub dt_us: i64, pub transpose: i8 }
```

- One `checkpoint()` for the whole action (one undo reverts every target;
  exactly one new rev). If no target applies, do **not** checkpoint (no-op).
- Unknown rev → `ActionError::BadParams` with `unknown_revision`.
- Add catalog/help entries and extend the `action.rs` parity tests.

### 4. Docs

`docs/AGENT-CONTROL.md`: a worked agent recipe — note the rev (`r41`), edit,
`FindSimilar {from_rev: 41}`, inspect/filter matches, `ApplyEditDiff` with the
chosen targets, verify via `EditDiff {from_rev: <rev before apply>}`.

## Tests

Build a fixture timeline with a 4-note phrase repeated at bars 1, 3, 5 and a
transposed copy (+5) at bar 7, plus a near-copy at bar 9 missing one note.

- Edit bar 1 (delete one note, change one velocity, add one note). With
  defaults, `find_similar` returns bars 3 and 5 (not bar 1, not 7, not 9), both
  `applicable`.
- `allow_transpose` also returns bar 7 with `transpose == 5`.
- `min_score_permille: 750` also returns bar 9; it is `!applicable` when the
  missing note is one the diff removes/changes, with a conflict message.
- Target notes 10 ms late keep their 10 ms offset after a change (deltas).
- `ApplyEditDiff` over bars 3, 5, 9 applies 3 and 5, skips 9, creates exactly
  one new rev; one `undo` restores the pre-apply state.
- `EmptyPattern`, `EmptyDiff`, `DiffTooWide` (a `nudge_tail` diff) errors.
- Pitch overflow at the top of the keyboard → conflict, target skipped.
- Control: `FindSimilar` JSON round-trip through `handle`; `apply_edit_diff`
  dispatches via `action_from_name`.

## Scope boundaries (do NOT)

- No keybindings or UI in either frontend — this is an agent-facing capability.
- No fuzzy rhythm matching beyond `tolerance_us` (no time-stretch, no
  inversion/retrograde).
- Notes only: grid/tempo, backgrounds and hand-split edits are not replicated.
- Do not change M19-A's revision semantics.
- Do not add third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #317`
