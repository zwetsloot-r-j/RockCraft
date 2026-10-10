# M19-A — Revision IDs, edit history & edit-diff queries

> Milestone: M19 — Edit replication · Issue: #316 · Suggested tier: sonnet
> Branch: `claude/m19-revision-ids`

## Goal

Give every editing state a stable numeric **revision ID** (a simple counter),
show it in the edit window of both frontends, and let an agent (a) list the
revisions in the undo/redo history and (b) get the exact note-level **diff**
between any two of them. Referring to "the state at rev 41" is more reliable
than counting edit operations (`undo` × n) and is the foundation for M19-B
(apply a set of edits to every similar passage).

## Context

- `crates/core/src/history.rs` — `History` stores whole `Timeline` snapshots:
  `past` (undo stack), `current`, `future` (redo stack), capacity
  `HISTORY_CAPACITY = 100` (`composer.rs`). `checkpoint()` is called *before*
  every mutation; `rollback()` cancels chord-selector previews; the
  `*_all` re-time helpers mutate every snapshot in place (tempo edits are
  deliberately **not** undo steps).
- `Timeline` keeps notes in a `BTreeMap<u32, Note>` with **stable `NoteId`s**
  that survive cloning into snapshots, so a diff between two snapshots can pair
  notes exactly by id.
- `ComposerSnapshot` (`composer.rs`) is what `query State` and every
  `run_action` reply return; `NoteView` already carries the note id.
- Queries: `QueryKind` + `Response` in `crates/control/src/protocol.rs`,
  dispatched in `handle`; the `hello` banner lists query kinds.
- Status bars: `crates/tui/src/edit.rs` (80-column budget — be terse) and
  `tauri-app/src/screens/edit/StatusBar.tsx`.

## What to do

### 1. Revision IDs in `History` (core)

Each stored state carries a `u64` revision. Rules:

- `History::new` → current is rev **0**; the next rev to allocate is 1.
- `checkpoint()` → pushes `(current_rev, current.clone())` onto `past`, then
  gives `current` a **fresh** rev (`next_rev`, then increment). Clears `future`.
- `undo()` / `redo()` → the restored state brings back **its own** rev (revs
  travel with their states, so the same content always has the same id).
- `rollback()` → restores the previous state *and its rev*. The counter is
  **not** decremented: a rev id is **never reused** within a `History`, so an
  id an agent holds can never silently point at different content.
- Capacity eviction drops the oldest `(rev, timeline)` pair.
- `scale_all_times` / `retempo_*_all` keep every rev unchanged (they re-time
  all states consistently, so diffs between revs stay meaningful).

```rust
// crates/core/src/history.rs
pub fn revision(&self) -> u64;                       // current rev
pub fn revisions(&self) -> Vec<RevisionInfo>;        // past (oldest first), current, future (next-to-redo first)
pub fn at_revision(&self, rev: u64) -> Option<&Timeline>; // any rev still in past/current/future

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RevisionInfo {
    pub rev: u64,
    pub position: RevisionPosition, // Past | Current | Future (snake_case on the wire)
    pub note_count: usize,
}
```

Add `pub revision: u64` to `ComposerSnapshot` (`#[serde(default)]`).

Loading a new piece builds a new `History`, so ids restart at 0 — that is
fine; document that revs are scoped to the currently loaded piece.

### 2. Note-level diff (core, pure)

New module `crates/core/src/edit_diff.rs`:

```rust
pub struct EditDiff {
    pub from_rev: u64,
    pub to_rev: u64,
    pub added: Vec<NoteView>,          // ids in `to` but not `from`
    pub removed: Vec<NoteView>,        // ids in `from` but not `to`
    pub changed: Vec<NoteChange>,      // same id, any field differs
    pub bounds: Option<DiffBounds>,    // None when the diff is empty
}
pub struct NoteChange { pub before: NoteView, pub after: NoteView }
/// Bounding box of every touched note (removed, added, and both sides of each
/// change): onset range [us_lo, us_hi) where us_hi = max(start+dur), pitch range inclusive.
pub struct DiffBounds { pub us_lo: u64, pub us_hi: u64, pub pitch_lo: u8, pub pitch_hi: u8 }

pub fn diff_timelines(from: &Timeline, to: &Timeline) -> (Vec<NoteView>, Vec<NoteView>, Vec<NoteChange>);
```

All lists sorted by `(start_us, pitch, id)` for deterministic output. All types
`Serialize`/`Deserialize`. An op that rebuilds notes with new ids shows up as
remove + add — still correct, just less compact; don't try to be clever.

Expose on `Composer`: `pub fn edit_diff(&self, from_rev: u64, to_rev: Option<u64>) -> Result<EditDiff, UnknownRevision>`
(`to_rev` defaults to the current rev).

### 3. Control queries

Add to `QueryKind` (struct variants serialise externally tagged, e.g.
`{"type":"query","what":{"EditDiff":{"from_rev":41}}}`):

```rust
History,                                       // → Response::History { current: u64, revisions: Vec<RevisionInfo> }
EditDiff { from_rev: u64, #[serde(default)] to_rev: Option<u64> }, // → Response::EditDiff(EditDiff)
```

An unknown/evicted rev → `Response::Err` with `unknown_revision: <rev>`.
Update the `hello` banner's `queries` list. Both socket servers (TUI and Tauri)
must answer the new queries — they share `control::protocol::handle`; verify.

### 4. Show the rev in the edit window

- TUI status bar: a compact `r42` (stay within the 80-column budget; follow the
  existing width-critical layout comments in `edit.rs`).
- Tauri `StatusBar.tsx`: `rev 42` from `snapshot.revision`.

### 5. Docs

`docs/AGENT-CONTROL.md`: document `revision` in the state snapshot, the
`History` and `EditDiff` queries, the struct-variant query shape, and the
"revs never reused / scoped to the loaded piece" rules.

## Tests

- `history.rs`: fresh history is rev 0; three checkpoints → revs 1,2,3;
  undo ×2 → current rev 1, `revisions()` shows 0 past, 1 current, 2,3 future;
  redo → rev 2; a new checkpoint after undo allocates rev **4** (not 3);
  `rollback()` restores the prior rev and the next checkpoint still allocates a
  fresh, never-seen id; eviction at capacity drops the oldest rev from
  `revisions()` and `at_revision` returns `None` for it; `scale_all_times`
  leaves revs unchanged.
- `edit_diff.rs`: add-only, remove-only, velocity/duration/hand change, and a
  mixed diff produce the expected lists and bounds; identical timelines →
  empty lists, `bounds == None`; output order is deterministic.
- Composer: `add_note` then `resize_note` → `edit_diff(rev_before, None)` shows
  one added note with the resized duration; a chord preview cancelled with
  `cancel_chord` leaves the rev unchanged.
- Control: `History` and `EditDiff` round-trip through `handle` (incl. the JSON
  wire shape above) and an unknown rev returns the `unknown_revision` error.
- Snapshot JSON without `revision` still deserialises (`serde(default)`).

## Scope boundaries (do NOT)

- Do not implement similarity search or applying diffs — that is M19-B.
- Do not change undo semantics (what is/isn't an undo step) or the capacity.
- Do not persist history or revs to disk.
- Do not add third-party dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #316`
