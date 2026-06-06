# M4-I — docs: agent control protocol guide + example client

> Milestone: M4 — Agent Interface · Issue: #93 · Suggested tier: cheap
> Branch: `claude/m4-agent-docs`

## Goal

Document the WebSocket control interface so an AI agent (or a human) can connect
to a running RockCraft and drive edits, plus ship a tiny worked example. Without
this, the interface exists but isn't discoverable.

## Context

- `docs/AGENT-CONTROL.md` (new), referenced from `CLAUDE.md`'s status section and
  `docs/WORKFLOW.md`. Builds on M4-D…G (#88–#91): the protocol, the server flag,
  the state/render queries.
- The action vocabulary is `core::action_names()` — the doc should point at that
  as the live source of truth rather than hand-maintaining a divergent list.

## What to do

- Write `docs/AGENT-CONTROL.md` covering:
  - how to start the app with the control server (`--control` / env, default
    localhost addr, how the bound port is printed);
  - the message shapes from M4-D (`run_action`, `query state|actions|render`,
    `subscribe events`) with a concrete request/response example each;
  - the verification loop: `run_action` → read the returned `state` (and/or
    `query render` for the text screenshot) → assert;
  - that `query actions` enumerates everything callable, so new actions are
    available with no doc/protocol change.
- Add one minimal example client under `crates/control/examples/` (e.g.
  `agent_session.rs`): connect, `add_note`, move cursor, `add_note`,
  `query state`, print the snapshot. Keep it dependency-light (reuse the crate's
  existing ws client deps).
- Add the pointer line to `CLAUDE.md` ("Current status") and `docs/WORKFLOW.md`.

## Tests

- Doc/example task: the example must compile (`cargo build --examples -p
  rockcraft-control`). No new unit tests required, but the example running
  against a live server should be described in the doc.

## Scope boundaries (do NOT)

- Do not restate `core`/architecture invariants already in `CLAUDE.md`; link them.
- Do not change the protocol or any code behaviour; this is docs + an example.
- Do not hand-maintain an action list that can drift from `action_names()`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green (`cargo build --examples` covers the example)
- [ ] PR against `main` from `claude/m4-agent-docs`, `Closes #93`
