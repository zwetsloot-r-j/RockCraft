# M4-F — tui: wire the running app to the control server

> Milestone: M4 — Agent Interface · Issue: #91 · Suggested tier: opus
> Branch: `claude/m4-app-control-wiring`

## Goal

Let the live TUI app be driven by the WebSocket server **and** the keyboard at
once, against a single source of truth, without ever blocking the render/MIDI
path. Remote `run_action`s and human keypresses both mutate the one `Composer`
the app owns; each surface sees the other's edits.

## Context

- Crates: `crates/tui` (the app shell / run loop) + `crates/control` (#90).
- The TUI owns the `Composer` (inside `EditScreen`, post M4-C). The control
  server runs on a tokio task. They communicate over a **channel**, not a shared
  lock on the app's composer — the render loop must never await on the socket and
  the socket task must never block the render loop.

## What to do

- Define a command seam the server sends into the app loop:

  ```rust
  // a Request plus a oneshot sender for its Response
  struct RemoteCommand { req: control::Request, reply: oneshot::Sender<control::Response> }
  ```

  The control server (M4-E variant) is constructed with the `mpsc::Sender<RemoteCommand>`
  instead of an `Arc<Mutex<Composer>>`: on each message it sends a `RemoteCommand`
  and awaits the `reply`.
- The app run loop drains the `mpsc::Receiver<RemoteCommand>` once per iteration
  (non-blocking `try_recv` loop), calls `control::handle(self.edit.composer_mut(),
  req)`, runs any returned `Effect`s through the synth interpreter (so remote
  edits audition just like key edits), and sends the `Response` back over the
  oneshot. A remote mutation triggers the same redraw path as a keypress.
- Add an opt-in flag/env (e.g. `--control` / `ROCKCRAFT_CONTROL_ADDR`,
  default 127.0.0.1:0 or a fixed dev port) that starts the server; off by
  default so normal runs open no socket. Print the bound address on startup so
  an agent/test can find the port.
- Spawn the tokio runtime/server thread separately from the terminal/MIDI
  threads; on quit, signal shutdown and join cleanly.

## Tests (headless — no real terminal, no real socket required)

- A headless harness that pumps the `RemoteCommand` channel directly (bypassing
  the socket) proves: a remote `add_note` and a scripted keypress edit converge
  on the same `Composer` (note count reflects both; a follow-up `query state`
  returns the merged state).
- Interleaving order is deterministic: commands drained per loop iteration apply
  in receive order; the oneshot reply carries the post-edit snapshot.
- Shutdown signal stops the server task without hanging the loop.

## Scope boundaries (do NOT)

- Do not hold a lock on the composer across `.await`, and do not let the socket
  task touch the composer directly — everything goes through the channel.
- Do not change the wire protocol (M4-D) or `core`. Render-buffer/state query
  *content* is M4-G; here just route the requests.
- Keep `tokio` confined to the server thread; the render/MIDI loop stays sync.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m4-app-control-wiring`, `Closes #91`
