# M4-E — control: localhost WebSocket server loop

> Milestone: M4 — Agent Interface · Issue: #90 · Suggested tier: opus
> Branch: `claude/m4-control-server`

## Goal

Turn the M4-D protocol into a running WebSocket server bound to localhost. The
server accepts connections, parses each text message into a `Request`, applies
it through the M4-D handler against a shared `Composer`, and writes back the
`Response`. This is the standalone, self-driving server; wiring it into the live
TUI app (one shared source of truth, no RT-thread blocking) is M4-F.

## Context

- Crate: `crates/control`. Builds on the protocol + `handle` from M4-D (#89).
- **New deps (this spec authorises them):** `tokio` (rt-multi-thread, macros,
  sync, net), `tokio-tungstenite`, `futures-util`. Add to
  `[workspace.dependencies]` and the crate. These are the project's first async
  deps — keep them confined to `crates/control`.
- The composer is the single mutable resource; serialise access. For this
  standalone server an `Arc<tokio::sync::Mutex<Composer>>` is fine. M4-F replaces
  that with a channel into the app's owned composer.

## What to do

```rust
// crates/control/src/server.rs
pub struct ControlServer { /* listener addr, shared state */ }

impl ControlServer {
    /// Bind to `addr` (callers pass "127.0.0.1:0" to get an OS-assigned port).
    pub async fn bind(addr: &str, composer: Arc<Mutex<Composer>>) -> io::Result<Self>;
    /// The actually-bound local address (port may have been 0).
    pub fn local_addr(&self) -> SocketAddr;
    /// Accept connections until cancelled; each message → `handle` → response.
    pub async fn serve(self) -> io::Result<()>;
}
```

- **Bind localhost only.** Reject/ignore non-loopback binds; this is a dev/test
  control channel, no auth by design — document that.
- Per connection: read text messages, `serde_json::from_str::<Request>`; on parse
  error reply `Response::Err { id: None, error: "bad_request: ..." }` (don't drop
  the socket). Lock the composer only for the duration of `handle`, never across
  an `.await` that waits on I/O — keep the critical section tiny.
- `Query::Render` may still be a placeholder here; M4-G supplies real content.
- Provide a graceful shutdown handle (e.g. accept a `CancellationToken` or return
  a `JoinHandle` + shutdown channel) so M4-F and tests can stop it cleanly.

## Tests (control crate, real loopback socket)

- Spin up `ControlServer::bind("127.0.0.1:0", ..)`, `serve` on a task, connect a
  `tokio-tungstenite` client:
  - send `{"type":"run_action","action":"add_note"}` → receive `Ok` whose `state`
    shows `notes.len() == 1`.
  - send a second `add_note` at a moved cursor → 2 notes; send `{"type":"query",
    "what":"state"}` and assert the snapshot matches.
  - malformed JSON → `Err` with `bad_request:` and the socket stays open.
  - two sequential clients observe the *same* composer state (shared resource).

## Scope boundaries (do NOT)

- Do not wire into `crates/tui` (M4-F). Do not bind non-localhost. No auth/TLS.
- Do not add file load/save. Do not change the M4-D protocol types or `core`.
- Keep async deps inside `crates/control`; do not leak `tokio` into `core`/`tui`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m4-control-server`, `Closes #90`
