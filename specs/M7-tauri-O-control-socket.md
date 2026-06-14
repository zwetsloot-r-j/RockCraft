# M7-tauri-O — tauri: expose the WebSocket control socket from the desktop app

> Milestone: M7 · Issue: #193 · Suggested tier: opus
> Branch: `claude/tauri-control-socket`
> Depends on: M4-E (#90, control server), M4-F (#91, TUI control wiring), M7-tauri-A (#160, IPC bridge)

## Goal

Let an external agent drive the **Tauri** app over the same localhost WebSocket
control protocol that the TUI already exposes, against the one live `Composer`
the Tauri backend owns — so `cargo run` of the desktop app (with the flag/env)
opens a `ws://127.0.0.1:<port>` that speaks `run_action` / `query state` /
`query help`, identical to the TUI.

Today the socket lives only in the TUI: `crates/tui/src/main.rs` depends on
`rockcraft-control` and starts `CommandServer` on `--control` /
`ROCKCRAFT_CONTROL_ADDR`. The Tauri backend deliberately does **not** depend on
`rockcraft-control` — it re-exposes the same verbs over Tauri's internal IPC
`invoke` only (`tauri-app/src-tauri/src/state.rs`, `lib.rs`), reachable from its
own webview but not over a socket. This task closes that gap.

## Context

- `crates/control` — `CommandServer::bind(addr, mpsc::Sender<RemoteCommand>)`,
  where `RemoteCommand { req: Request, reply: oneshot::Sender<Response> }`
  (`crates/control/src/command.rs`). The server forwards each socket request to
  the app over the channel and awaits the reply; it never owns the composer.
  `protocol::handle(&mut Composer, Request) -> Response` is the pure dispatcher.
- TUI precedent to mirror: `crates/tui/src/main.rs` (`start_control_server`,
  the loopback-only bind, the stderr banner, draining the receiver each loop
  iteration via `try_recv`). Reuse the same env/flag contract.
- Tauri backend: `tauri-app/src-tauri/src/state.rs` owns `AppState { composer:
  Mutex<Composer>, … }`; `run_action`/`query_state`/`query_help` free functions
  already wrap `action_from_name` + `Composer::apply` + `action_help`. The Tauri
  tick thread (`lib.rs`, `TICK_PERIOD`) already advances the transport and emits
  `snapshot`/`effects` events to the webview.
- `docs/AGENT-CONTROL.md` documents the protocol and the `--control` /
  `ROCKCRAFT_CONTROL_ADDR` contract — currently TUI-only in wording.

## What to do

- Add `rockcraft-control = { workspace = true }` and the `tokio` runtime needed
  to run `CommandServer` to `tauri-app/src-tauri/Cargo.toml`. (Tauri already
  pulls a tokio runtime transitively; prefer reusing Tauri's `async_runtime`
  over spawning a second runtime — see below.)
- Start the server when `--control` is passed **or** `ROCKCRAFT_CONTROL_ADDR`
  is set, off by default, loopback-only (refuse non-loopback binds exactly as
  the server already does). Print `Control server listening on ws://<addr>` to
  **stderr** on bind, matching the TUI banner so `docs/AGENT-CONTROL.md` stays
  accurate for both frontends.
- Bridge the socket to `AppState` **without** a second source of truth. Two
  acceptable designs — pick the one that fits the Tauri thread model and justify
  it in the PR:
  1. **Channel seam (mirrors the TUI).** Hand `CommandServer` an
     `mpsc::Sender<RemoteCommand>`; in the Tauri tick thread, drain the receiver
     each tick with `try_recv`, run each `req` via the **same** `state::run_action`
     / `state::query_*` path (so effects route to the synth and the backing
     transport syncs exactly like an IPC call and a keypress), and answer the
     `oneshot`. After applying a remote mutation, emit the `snapshot`/`effects`
     events so the webview stays in sync — a socket edit must look identical to
     an IPC edit.
  2. **Shared lock.** Give the server task a handle that locks
     `AppState.composer` directly. Only choose this if it can be done without
     ever blocking the Tauri tick/UI thread on the socket; the channel seam is
     preferred because it reuses the existing `state::run_action` effect/event
     plumbing rather than duplicating it.
- Keep the IPC `invoke` commands working unchanged: the socket is an *additional*
  surface over the same state, not a replacement. A socket `run_action` and a
  webview `invoke("run_action", …)` interleaved must both see each other's edits
  and both emit the webview events.
- Update `docs/AGENT-CONTROL.md` so the "Start it" section covers the Tauri
  binary too (`cargo run --bin rockcraft-tauri -- --control`, same env var).

## Tests

- A headless integration test (no webview) that: builds an `AppState`, starts the
  control bridge against an OS-assigned loopback port, connects a WS client,
  sends `query Help` and asserts the action catalog is non-empty and equals
  `core::action_help()`; sends `run_action add_note` then `query State` and
  asserts the note count increased. Mirror `crates/control/tests/demo_scenario.rs`
  and `crates/tui` control tests.
- A test asserting an interleaved socket `run_action` and a direct
  `state::run_action` call share one `Composer` (edits from each are visible to
  the other).
- A test asserting a non-loopback `ROCKCRAFT_CONTROL_ADDR` is refused.

## Scope boundaries (do NOT)

- Do **not** add new protocol verbs or new actions here — this is wiring the
  *existing* protocol into Tauri. App-level workflow commands (play/record/
  import/library/backing) are the separate M8-A host-command tier; do not
  smuggle them in.
- Do **not** change `crates/core` or the `Action` catalog.
- Do **not** weaken the loopback-only restriction or change the default-off
  behaviour.
- Do **not** spin up a second tokio runtime if Tauri's `async_runtime` can host
  the server task; avoid duplicate runtimes.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `cargo run --bin rockcraft-tauri -- --control` prints the ws banner and a
      WS client can `query help` + `run_action` against it
- [ ] `docs/AGENT-CONTROL.md` updated to cover the Tauri binary
- [ ] PR opened against `main` from the branch above, `Closes #193`
