# M0 — Echo

> Milestone: M0 — Echo · Issue: #TBD
> The first hardware + TUI + dependency milestone.

## Goal

Prove the end-to-end input path: `midir` reads the connected USB-MIDI piano, a
`ratatui` screen lists incoming note events live (note name, on/off, velocity,
timestamp), and the session is recorded to a `.mid` file that replays correctly
via `midly`. No scoring, no highway yet.

This milestone is also our first real test that the cloud sandboxes can fetch
crates from crates.io (it adds the first third-party deps).

## Why it splits across local and remote

- **Local-only (needs the piano), `agent:human` + `loc:local`:** the live
  `midir` device loop, latency/feel, confirming events arrive correctly.
- **Sandbox-safe, delegatable:** the pure parts in `core` — an in-memory event
  buffer/timeline and the record→`.mid` serialization + a `.mid`→events parse,
  all testable against synthetic/recorded fixtures with no device.

## Proposed task breakdown (each → its own issue + spec)

1. `core`: event buffer / recording timeline type (pure). **Remote OK.**
2. `midi`: `midly` write (events → `.mid`) and read (`.mid` → events), tested
   round-trip against fixtures. **Remote OK.**
3. `midi`: `midir` live input loop emitting `core::NoteEvent`. **Local-only.**
4. `tui`: `ratatui` view listing live events. **Local-only** (to feel it), though
   the rendering logic can be unit-tested headless.

Dependencies introduced here (add only in the crate that needs them):
`midir`, `midly` (crate `midi`); `ratatui`, `crossterm` (crate `tui`).

## Acceptance (milestone-level)

- [ ] Piano note-ons/offs appear in the TUI in real time.
- [ ] A recorded `.mid` replays to the same event sequence (round-trip test).
- [ ] Each sub-task PR passes the gate; deps added only where used.
