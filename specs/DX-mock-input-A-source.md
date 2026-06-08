# DX — mock keyboard input (swappable NoteSource)

> Milestone: DX / dev-tooling · Issue: #39 · Suggested tier: opus
> Branch: `claude/mock-input-source`

## Goal

Let the TUI run and be developed **without a piano**. Abstract the event source
behind a trait, and provide a `MockKeyboard` source that turns computer-keyboard
hotkeys into `NoteEvent`s. This unblocks UI work away from the piano and is the
foundation for headless TUI tests (see `specs/DX-mock-input-B-tests.md`, #40).

## Context

- Today the source is the concrete `LiveInput` (`crates/midi/src/live.rs`).
  `Shell` (`crates/tui/src/app.rs`) holds it by type and the run loop drains it
  once per frame: `let events: Vec<_> = shell.input.events().collect();`
  (`app.rs:141`).
- `parse_note_message` is already pure/tested; only `LiveInput::connect` needs
  hardware. The only real coupling is `Shell` naming the concrete type.
- Read `CLAUDE.md` for invariants: `core` stays pure (the trait lives in `midi`,
  not `core` — a wall clock is not pure-domain); never block the real-time
  thread (`LiveInput` keeps its existing channel design unchanged).

## What to do

**1. Define the trait in `crates/midi`** (e.g. `crates/midi/src/source.rs`,
re-exported from `lib.rs`):

```rust
pub trait NoteSource {
    /// Note events received since the last call (non-blocking, may be empty).
    fn events(&mut self) -> Vec<NoteEvent>;
    /// Human-readable source name, shown in the menu header.
    fn port_name(&self) -> &str;
}
```

Implement `NoteSource` for `LiveInput` (drain `self.receiver.try_iter()`;
`port_name` returns the existing field). `events` taking `&mut self` is fine —
`Shell` owns the source.

**2. `MockKeyboard` source** (`crates/midi/src/mock.rs`):

```rust
pub struct MockKeyboard { /* clock origin, pending event queue */ }

impl MockKeyboard {
    pub fn new() -> Self;
    /// Map a typed character to a MIDI note and enqueue a note-on now plus a
    /// note-off SUSTAIN_MS later. Returns the note struck, or None if the key
    /// is unmapped. Timestamps are microseconds since `new()` (monotonic).
    pub fn press(&mut self, key: char) -> Option<MidiNote>;
}
```

- Keyboard→note map: the **number row** `1 2 3 4 5 6 7 8 9 0` → a C-major
  (white-key) scale C D E F G A B C D E from C4 (note 60). Spell the exact map in
  code with a comment; pin it in tests. (Updated by #124: the original home-row
  `a s d f …` map collided with the editor's letter command keys; the digit row
  is disjoint from those commands. Shift is avoided — crossterm delivers `Shift+1`
  as `!` — so the plain digit row is the distinguishable choice.)
- Terminals don't reliably deliver key **release**, so each `press` enqueues a
  note-on at `now_us` and a note-off at `now_us + SUSTAIN_MS*1000`
  (`SUSTAIN_MS` ≈ 120). `events()` returns only events whose timestamp has
  arrived (drains the queue up to the current clock), so a held/repeated key
  produces clean on/off pairs and scoring stays deterministic.
- Use `std::time::Instant` for the clock; no third-party deps.

**3. Wire into the TUI** (`crates/tui`):

- `Shell` holds `Box<dyn NoteSource>` instead of `LiveInput`. The run loop and
  `draw_menu` call `events()` / `port_name()` through the trait — no other
  screen logic changes.
- Source selection in `main.rs`: pass `--mock` to force `MockKeyboard`; and when
  `LiveInput::connect` fails to find a port, **fall back to `MockKeyboard`**
  (print a one-line notice) instead of `exit(1)`, so the app always launches.
- Key routing in `on_key`: when the active source is the mock, a press of a
  mapped note key inside the Record/Play screens is forwarded to the mock's
  `press(..)`. Reserve `Tab`/`Esc`/`Enter`/arrows for navigation in all modes;
  the Menu does not take note input. Resolve the overlap between note letters
  and existing screen controls (`s` save, `r`/`m` play) by only treating
  letters as notes inside a screen, and document the precedence in a comment.

Mock note-ons must flow through the same path as live events (so the synth
sounds them and Record/Play ingest them) — i.e. they arrive via `events()` next
frame, not by calling screen methods directly.

## Tests

In `crates/midi` (headless, no hardware):

- `press('1')` returns `Some(C4=60)`; an unmapped key (e.g. a letter) returns `None`.
- After a `press`, `events()` eventually yields a note-on then a note-off for
  the same pitch (drive/inspect via the clock or a small seam); the off
  timestamp is `SUSTAIN_MS` after the on.
- The full keyboard→note map is asserted for the documented keys.
- `LiveInput`'s `NoteSource` impl compiles and `parse_note_message` tests still
  pass unchanged.

## Scope boundaries (do NOT)

- No `core` changes; no changes to `NoteEvent`/scoring types.
- Do not alter `parse_note_message` behaviour or `LiveInput`'s channel design.
- No new third-party dependencies.
- Do not build the scripted-replay test harness here — that is #40.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/mock-input-source`, `Closes #39`
