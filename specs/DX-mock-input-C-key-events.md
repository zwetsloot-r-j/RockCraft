# DX — injectable KeySource for end-to-end key-driven tests

> Milestone: DX / dev-tooling · Issue: #42 · Suggested tier: sonnet
> Branch: `claude/mock-input-keys`

## Goal

Drive the TUI through its **real** key-handling path from a test — simulated
keystrokes flow through `on_key` and the run loop exactly as live ones do. This
lets automated tests cover menu navigation and the `MockKeyboard` char→note
mapping, which `ScriptedSource` (#40) deliberately bypasses by injecting
`NoteEvent`s at the source level.

## Context

- Depends on **#39** (`on_key` routing + `MockKeyboard`) and complements **#40**
  (`ScriptedSource` + `TestBackend`). Read both specs first.
- Today the run loop reads keys inline (`crates/tui/src/app.rs:171`):

  ```rust
  if event::poll(Duration::from_millis(16))? {
      if let Event::Key(key) = event::read()? {
          if key.kind == KeyEventKind::Press {
              shell.on_key(key.code);
          }
      }
  }
  ```

  There is no seam to feed synthetic keys — that is what this task adds.
- `KeyCode` is a crossterm type and the seam is a terminal concern, so the trait
  lives in `crates/tui`. No `core`/`midi` changes.

## What to do

**1. `KeySource` trait** (`crates/tui`):

```rust
pub trait KeySource {
    /// Return the next key press if one is available within `timeout`,
    /// `None` on timeout. Only key-DOWN presses are surfaced.
    fn poll_key(&mut self, timeout: Duration) -> io::Result<Option<KeyCode>>;
}
```

- `CrosstermKeys`: wraps the current `event::poll`/`event::read` +
  `KeyEventKind::Press` filter. Production default.
- `ScriptedKeys`: a queue of `KeyCode`s (optionally with a per-key frame/delay
  budget) returned one per `poll_key`; yields `None` once drained so the loop
  can settle.

**2. Thread it through the loop.** `run_loop` takes `&mut dyn KeySource`
(or a generic). `run()` wires `CrosstermKeys`. Keep this behaviour-preserving —
no change to live behaviour. Pair with the `NoteSource` seam from #39 so a test
can construct a `Shell` from a `ScriptedKeys` + a note source (`ScriptedSource`
or `MockKeyboard`) and step frames against a `TestBackend`.

**3. End-to-end tests** (`crates/tui/tests/` or `#[cfg(test)]`):

- **Menu navigation**: feed ↓/Enter to open Record, then Tab/Esc to return to
  Menu; assert the active screen transitions accordingly.
- **Mock note mapping**: in mock mode, feed mapped note keys inside Record and
  assert the corresponding `NoteEvent`s reach the recording timeline / event log
  (i.e. `MockKeyboard.press` fired through `on_key`, not called directly).
- **Control vs. note precedence**: confirm reserved keys (Tab/Esc/Enter/arrows)
  navigate and never produce notes, while letter keys produce notes only inside
  a screen — pin the precedence chosen in #39.
- **Quit**: `q`/Esc from the Menu sets `should_quit` and the loop exits.

## Tests

The end-to-end tests above are the deliverable. Also unit-test `ScriptedKeys`:
keys are returned in order, one per `poll_key`, and `None` after the queue
empties.

## Scope boundaries (do NOT)

- Depends on #39; do not re-implement `on_key` or the note map.
- `crates/tui` only. No `core`/`midi` changes beyond what #39 lands.
- No new third-party dependencies (`TestBackend` ships with ratatui).
- Do not weaken or delete existing tests; keep live key behaviour identical.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/mock-input-keys`, `Closes #42`
