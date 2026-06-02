# M2-C — Song/Recording bundle model + backing-track sync math (core)

> Milestone: M2 — Audio · Issue: #29 · Suggested tier: sonnet
> Branch: `task/song-bundle` (or `claude/song-bundle`)

## Goal

A **pure, headless** domain model for a recorded session that may carry a
backing audio track, plus the timing math that keeps the backing track in sync
with the falling-note highway. This is the contract Tasks D (#30, write) and E
(#31, read) build on, and the one M2 task a cloud/remote agent can fully build
and verify **with no hardware**.

## Context

- Crate: `crates/core`. Per `CLAUDE.md`, `core` is "events, song timeline,
  timing clock, scoring — NO I/O." A recording's *container description* and the
  *sync math* are timeline/timing domain and belong here. **All real file/dir
  I/O stays out of `core`** — Tasks D/E do `std::fs` in the tui layer.
- Today a recording is a bare `take-<stamp>.mid`. M2 introduces a **bundle
  directory** `recordings/take-<stamp>/` holding `song.mid` + (optional)
  `backing.<ext>` + `meta.json`. This task defines `meta.json`'s shape and the
  math; it does **not** read or write any files.
- Sync anchor (verified against `crates/tui/src/play.rs` + `highway.rs`): Play
  mode shifts every note by `shift = (PRE_ROLL_US + LEAD_US) − first_note_us`,
  and a note reaches the keyboard line when the playback clock equals its
  shifted start. So an original recording time `t` is heard at playback clock
  `t + shift`.

## What to do

New module `crates/core/src/song.rs` (export its public items from `lib.rs`).
Add `serde` (derive) + `serde_json` to `crates/core/Cargo.toml` — these are pure
data crates, no hardware/fs; they keep `core` headless-testable. **Do not** add
any other deps and **do not** add `std::fs` to `core`.

### Manifest types (serde)

```rust
/// Describes the backing audio inside a bundle. `file` is the bundle-relative
/// filename only (e.g. "backing.mp3") — never an absolute path, so the bundle
/// stays movable. `audio_start_us` is the position in the audio file that lines
/// up with recording time 0 (usually 0; nonzero allows a trimmed lead-in).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BackingTrack { pub file: String, pub audio_start_us: u64 }

/// The bundle manifest, serialized as meta.json.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecordingMeta {
    pub midi_file: String,            // e.g. "song.mid"
    pub backing: Option<BackingTrack>,
    // include a small version field for forward-compat, default to 1.
}

impl RecordingMeta {
    pub fn new_midi_only(midi_file: impl Into<String>) -> Self;
    /// Pure string<->struct, no I/O. from_json maps errors to a typed error.
    pub fn to_json(&self) -> String;
    pub fn from_json(s: &str) -> Result<Self, MetaError>;
}
```

### Sync math (pure functions)

```rust
/// The whole-song forward shift Play mode applies. Mirrors play.rs:
/// (pre_roll_us + lead_us).saturating_sub(first_note_us).
pub fn song_shift_us(first_note_us: u64, pre_roll_us: u64, lead_us: u64) -> u64;

/// Where in the backing file to be at a given playback-clock time.
/// Returns None while the clock is before `shift_us` (audio not started yet);
/// otherwise Some((clock_us - shift_us) + audio_start_us).
pub fn backing_position_us(clock_us: u64, shift_us: u64, audio_start_us: u64) -> Option<u64>;
```

## Tests (headless, exhaustive — this is the verifiable task)

- `RecordingMeta` round-trips through `to_json`/`from_json` for both midi-only
  and with-backing variants (assert structural equality).
- `from_json` on malformed/foreign JSON returns `Err`, not panic.
- A manifest written by an older minimal `meta.json` (just `midi_file`) still
  deserializes (backing defaults to `None`).
- `song_shift_us`: `first=0,pre=1_500_000,lead=2_000_000 → 3_500_000`;
  `first` larger than `pre+lead` → `0` (saturating).
- `backing_position_us`:
  - clock < shift → `None`.
  - clock == shift, audio_start 0 → `Some(0)`.
  - clock == shift + 1_000_000, audio_start 0 → `Some(1_000_000)`.
  - clock == shift, audio_start 250_000 → `Some(250_000)`.

## Scope boundaries (do NOT)

- `crates/core` only. **No `std::fs`, no path traversal, no reading `.mid`/audio
  bytes** — only the manifest's own JSON string and integer math.
- Only `serde` + `serde_json` may be added. No other deps.
- Do not change existing `core` public signatures; only add `song` items.
- Do not implement bundle reading/writing or audio playback — that is D/E.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green (the headless tests above)
- [ ] PR against `main` from the branch above, `Closes #29`
