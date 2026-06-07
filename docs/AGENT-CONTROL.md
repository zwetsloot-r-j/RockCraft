# Agent Control Protocol

> **Status:** Stable (M4) — see [CLAUDE.md](../CLAUDE.md) for architecture invariants.

This document describes the WebSocket control interface that lets an AI agent (or a human) connect to a running RockCraft instance and drive edits programmatically. The interface is built on the `rockcraft-control` crate and exposes the full action vocabulary from `core::action_names()`.

## Quick start

### Starting the control server

The control server is **localhost-only** and **unauthenticated** by design. It binds to `127.0.0.1` and refuses any non-loopback address.

```bash
# From the TUI (most common)
RUST_LOG=info cargo run --bin rockcraft-tui -- --control

# Or programmatically via the control crate
# The bound port is printed to stdout, e.g.:
# [INFO] Control server bound to 127.0.0.1:38473
```

You can also set the port via environment variable:

```bash
CONTROL_PORT=9001 cargo run --bin rockcraft-tui -- --control
```

If no port is specified, the OS assigns one; the actual address is logged and can be queried via the server's `local_addr()` method.

### Connecting a client

The server speaks plain WebSocket (no TLS). Connect to `ws://127.0.0.1:<PORT>`.

See [`crates/control/examples/agent_session.rs`](../../crates/control/examples/agent_session.rs) for a minimal working example.

## Protocol

The protocol is JSON-based over WebSocket text frames. Each message is a [`Request`] sent by the client and a [`Response`] returned by the server.

### Message shapes

#### Request types

All requests are JSON objects with a `type` field (snake_case) that determines the variant.

| Type | Purpose | Example |
|------|---------|---------|
| `run_action` | Execute a composer action | [Below](#run_action) |
| `query` | Query state, actions, or render | [Below](#query) |
| `subscribe` | Subscribe to events | [Below](#subscribe) |
| `unsubscribe` | Unsubscribe from events | [Below](#unsubscribe) |

##### `run_action`

Executes a single action from the [action vocabulary](#action-vocabulary). Returns the resulting state snapshot and any side effects.

**Request:**
```json
{
  "type": "run_action",
  "id": 1,
  "action": "add_note",
  "params": {}
}
```

**Response (success):**
```json
{
  "type": "ok",
  "id": 1,
  "effects": [
    {"effect": "audition_note", "pitch": 60, "velocity": 80}
  ],
  "state": { ... snapshot ... }
}
```

**Response (error):**
```json
{
  "type": "err",
  "id": 1,
  "error": "unknown_action: frobnicate"
}
```

The `id` field is optional and is echoed back in the response for correlation. Use it to match responses to requests when pipelining.

##### `query`

Queries the current state of the composer or enumerates available actions.

**Request (state):**
```json
{
  "type": "query",
  "id": 2,
  "what": "state"
}
```

**Response:**
```json
{
  "type": "ok",
  "id": 2,
  "effects": [],
  "state": { ... snapshot ... }
}
```

**Request (actions):**
```json
{
  "type": "query",
  "id": 3,
  "what": "actions"
}
```

**Response:**
```json
{
  "type": "actions",
  "id": 3,
  "actions": [
    "cursor_left", "cursor_right", "add_note", "delete_note",
    "resize_note", "cursor_up", "cursor_down", ...
  ]
}
```

**Request (render):**
```json
{
  "type": "query",
  "id": 4,
  "what": "render"
}
```

**Response:**
```json
{
  "type": "render",
  "id": 4,
  "text": "... terminal screenshot ..."
}
```

##### `subscribe`

Subscribe to event streams. Currently only `events` topic is supported.

**Request:**
```json
{
  "type": "subscribe",
  "id": 5,
  "topic": "events"
}
```

**Response:**
```json
{
  "type": "ok",
  "id": 5,
  "effects": [],
  "state": null
}
```

After subscribing, the server will send `Event` messages asynchronously:

```json
{
  "type": "event",
  "topic": "events",
  "event": { ... }
}
```

##### `unsubscribe`

**Request:**
```json
{
  "type": "unsubscribe",
  "id": 6,
  "topic": "events"
}
```

**Response:**
```json
{
  "type": "ok",
  "id": 6,
  "effects": [],
  "state": null
}
```

### The verification loop

The recommended pattern for agent-driven editing is:

1. **Run an action:** `run_action` with your desired operation
2. **Read the state:** The response includes a `state` snapshot — inspect it to verify the action's effect
3. **Optionally query render:** For a "text screenshot", use `query { what: "render" }` to get the terminal representation
4. **Assert:** Compare the returned state/render against your expectations

Example flow:

```
Client: run_action { action: "add_note", params: {} }
Server: ok { state: { cursor: { pitch: 60, step: 0 }, notes: [...] }, ... }

Client: query { what: "state" }
Server: ok { state: { cursor: { pitch: 60, step: 0 }, notes: [...] }, ... }

Client: query { what: "render" }
Server: render { text: "..." }
```

### Action vocabulary

The complete list of callable actions is available via `query { what: "actions" }`. **Do not hand-maintain a separate list** — the response from this query is the live source of truth, and it is guaranteed to stay in sync with `core::action_names()`.

Each action maps to a variant in `rockcraft_core::Action`. See the [core action module](../../crates/core/src/action.rs) for the full enumeration and parameter shapes.

Common actions for agent use:

| Action | Parameters | Description |
|--------|------------|-------------|
| `add_note` | none | Add a note at the current cursor position |
| `delete_note` | none | Delete the note under the cursor |
| `set_cursor` | `{ pitch: u8, step: u64 }` | Absolute cursor positioning |
| `cursor_left` / `cursor_right` / `cursor_up` / `cursor_down` | none | Relative cursor movement |
| `resize_note` | `{ delta_steps: i64 }` | Resize the note under cursor |
| `adjust_velocity` | `{ delta: i16 }` | Adjust note velocity |
| `undo` / `redo` | none | History navigation |

For the exhaustive list, call `query { what: "actions" }` or consult `core::action_names()`.

## Example session

See [`crates/control/examples/agent_session.rs`](../../crates/control/examples/agent_session.rs) for a complete, runnable example that:

1. Connects to a running control server
2. Adds a note
3. Moves the cursor
4. Adds another note
5. Queries the state
6. Prints the snapshot

To run the example:

```bash
# Terminal 1: Start the TUI with control server
cargo run --bin rockcraft-tui -- --control

# Terminal 2: Run the example (replace PORT with the logged port)
cargo run --example agent_session -- --port PORT
```

## State snapshot

The `state` field in responses is a `ComposerSnapshot` from `rockcraft_core`. It includes:

- `cursor`: Current cursor position (`pitch`, `step`, `subdivision`)
- `selection`: Active selection range (if any)
- `notes`: All notes in the timeline
- `timeline`: The full timeline structure
- `history`: Undo/redo stack info
- `playhead`: Current playhead position
- `is_playing`: Playback state
- `loop_bounds`: Loop start/end in microseconds
- `metronome_enabled`: Whether metronome is on
- `input_mode`: Current input mode

See `rockcraft_core::ComposerSnapshot` for the full schema.

## Error handling

All errors return a response of type `err` with an `error` string. Common errors:

| Error | Cause |
|-------|-------|
| `unknown_action: <name>` | The action name is not in `action_names()` |
| `bad_params: <action>: <detail>` | Parameters are missing, wrong type, or invalid |
| `bad_request: <detail>` | The JSON could not be parsed as a valid `Request` |
| `unavailable: control channel closed` | The application's command channel is closed |
| `unavailable: no reply from application` | The application did not respond to a forwarded request |

## Security note

**The control server is localhost-only and unauthenticated.** It is designed for development and testing. Do not expose it on a public network, and do not run untrusted agents against it — they can arbitrarily modify the composer state.

## See also

- [Development workflow](../WORKFLOW.md) — how work is tracked and delegated
- [CLAUDE.md](../CLAUDE.md) — architecture invariants and agent guide
- [`rockcraft-control` crate](../../crates/control/) — implementation of the server and protocol
- [`rockcraft-core` crate](../../crates/core/) — the pure composer engine and action vocabulary
