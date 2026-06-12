# Agent Control Protocol

> **Status:** Stable (M4) — see [CLAUDE.md](../CLAUDE.md) for architecture invariants.

This document describes the WebSocket control interface that lets an AI agent (or a human) connect to a running RockCraft instance and drive edits programmatically. The interface is built on the `rockcraft-control` crate and exposes two vocabularies:

- **Composer actions** (`run_action`) — the full action vocabulary from `core::action_names()`, applied to the pure `Composer`.
- **Host commands** (`run_command`) — app-level workflows (play / record / import / library / backing) dispatched by the frontend over its own services. See [Host-command tier](#host-command-tier-m8-a).

A single `query help` enumerates both.

## Quick start

### Starting the control server

The control server is **localhost-only** and **unauthenticated** by design. It binds to `127.0.0.1` and refuses any non-loopback address.

```bash
# From the TUI (most common). The bound address is printed to STDERR, e.g.:
#   Control server listening on ws://127.0.0.1:38473
cargo run --bin rockcraft-tui -- --control
```

You can pin a fixed `host:port` with the `ROCKCRAFT_CONTROL_ADDR` environment
variable. Setting it **also enables the server**, so `--control` becomes
optional — this is the recommended path for a scripted agent that needs a
known, stable address:

```bash
ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9001 cargo run --bin rockcraft-tui
```

If the address ends in `:0` (the default is `127.0.0.1:0`), the OS assigns the
port; read the actual address from the stderr line above, or from the server's
`local_addr()` when embedding the `rockcraft-control` crate directly.

The **Tauri desktop app** exposes the same socket with the same opt-in
(`--control` / `ROCKCRAFT_CONTROL_ADDR`), additive to its internal IPC commands.
It drives the one live `Composer` the backend owns, so connecting to it is
identical to connecting to the TUI — same banner, verbs, and `query help`
catalog. A remote action is reflected in the desktop webview (the backend emits
the same `snapshot` event the IPC path does), and so is a host command that
changes the composer (e.g. `library_load`).

```bash
# Same protocol, the desktop host instead of the TUI:
cargo run --bin rockcraft-tauri -- --control
```

> Both hosts speak the **composer-action** tier (`run_action`) **and** the
> **host-command** tier (`run_command`): play / record / import / library /
> backing. See [Host-command tier](#host-command-tier-m8-a).

### Connecting a client

The server speaks plain WebSocket (no TLS). Connect to `ws://127.0.0.1:<PORT>`.

See [`crates/control/examples/agent_session.rs`](../../crates/control/examples/agent_session.rs) for a minimal working example.

### The connection banner

Immediately after the handshake — **before you send anything** — the server
pushes one unsolicited `hello` frame. It exists so an agent that never read this
doc can still bootstrap: it names the request verbs and query kinds and points
at `query help`.

```json
{
  "type": "hello",
  "protocol": "rockcraft-control/1",
  "requests": ["run_action", "run_command", "query", "subscribe", "unsubscribe"],
  "queries": ["State", "Actions", "Help", "Render"],
  "hint": "send {\"type\":\"query\",\"what\":\"Help\"} for the full action + command catalog"
}
```

Because the banner (and any `event` you subscribe to) arrive **unsolicited**, a
correct client must not assume the next frame is the reply to its last request.
Correlate replies by skipping frames whose `type` is `hello` or `event` (and/or
matching the `id` you sent). The example client and the demo test both do this.

## Protocol

The protocol is JSON-based over WebSocket text frames. Each message is a [`Request`] sent by the client and a [`Response`] returned by the server.

### Message shapes

#### Request types

All requests are JSON objects with a `type` field (snake_case) that determines the variant.

| Type | Purpose | Example |
|------|---------|---------|
| `run_action` | Execute a composer action | [Below](#run_action) |
| `run_command` | Execute an app-level host command | [Below](#run_command) |
| `query` | Query state, actions, help, or render | [Below](#query) |
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

##### `run_command`

Executes a single **host command** from the [host-command vocabulary](#host-command-tier-m8-a) — an app-level workflow (play / record / import / library / backing) the frontend runs over its own services. Same call shape as `run_action`, but with a `command` name instead of an `action`.

**Request:**
```json
{
  "type": "run_command",
  "id": 10,
  "command": "library_save",
  "params": { "name": "my-take" }
}
```

**Response (success):**
```json
{
  "type": "command_ok",
  "id": 10,
  "data": { "saved": "/home/me/.rockcraft/library/my-take" }
}
```

`data` carries the command's own JSON result (a saved bundle path, the library
listing under `bundles`, …) and is omitted entirely for a nullary side effect
(e.g. `backing_detach`).

**Response (error):** a normal `err` with one of these prefixes:

| Prefix | Cause |
|--------|-------|
| `unknown_command: <name>` | Not in `host_command_names()`. |
| `bad_params: <command>: <detail>` | Params missing / wrong type. |
| `command_failed: <detail>` | The service rejected it — e.g. `record_save` with no active session, or `play_load` of a missing bundle. |

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

**Request (help):**
```json
{
  "type": "query",
  "id": 7,
  "what": "Help"
}
```

**Response:**
```json
{
  "type": "help",
  "id": 7,
  "actions": [
    {
      "name": "set_cursor",
      "params": [
        { "name": "pitch", "type": "u8" },
        { "name": "step", "type": "u64" }
      ],
      "description": "Absolute jump to a (pitch, step) cell — AI-friendly addressing."
    },
    {
      "name": "add_note",
      "params": [],
      "description": "Add a note at the cursor (duration 1 step, velocity 80); replaces any note already in that cell."
    }
  ],
  "commands": [
    {
      "name": "play_load",
      "params": [ { "name": "dir", "type": "string" } ],
      "description": "Load the bundle directory `dir` and start playing it on the note highway."
    },
    {
      "name": "library_scan",
      "params": [],
      "description": "Scan the library roots and return the bundles found."
    }
  ]
}
```

`help` is the structured superset of `actions`: it lists **every** action with
its parameter schema (`name` + Rust scalar `type`) and a one-line description
(from `core::action_help()`), **and** every host command under `commands` (from
`host_command_help()`). Query it once on connect to discover the full call shape
of both tiers — no hand-maintained table to consult. `commands` is absent only
when the catalog is empty, which never happens in practice.

> **Note on `what` casing:** `QueryKind` serialises with its Rust variant names,
> so the values are `State`, `Actions`, `Help`, `Render` (PascalCase), not
> snake_case.

**Request (render):**
```json
{
  "type": "query",
  "id": 4,
  "what": "Render"
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

Two queries describe the vocabulary live, so **nothing here needs hand-maintaining**:

- `query { what: "Actions" }` — just the names (`core::action_names()`).
- `query { what: "Help" }` — names **plus** parameter schema and a one-line
  description for each (`core::action_help()`). Prefer this when connecting cold:
  it tells you exactly what params each action takes.

Both are guaranteed in sync with the `Action` enum by parity tests in
`crates/core/src/action.rs`. Each action maps to a variant in
`rockcraft_core::Action`; see that module for the canonical definitions.

The small table below is an at-a-glance convenience only — `query help` is the
authoritative, exhaustive source:

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

## Host-command tier (M8-A)

Where an **action** edits the pure `Composer`, a **host command** drives an
app-level workflow that does I/O — load a song to play, run a record session,
attach a backing track, import from a URL, scan/save/load the library. These can
never be `core::Action`s (the `core` crate forbids I/O), so they form a parallel
vocabulary sent with `run_command` and catalogued under `commands` in
`query help`.

The catalog is live and self-describing — **prefer `query help` over this
table**, which is an at-a-glance convenience only:

| Command | Params | Description |
|---|---|---|
| `play_load` | `{ dir: string }` | Load a bundle and start playing it. |
| `play_stop` | none | Stop the active play session. |
| `record_start` | `{ backing?: string }` | Begin a record session (optional backing). |
| `record_stop` | none | Stop the session, discarding the take. |
| `record_save` | none | Save the session to a bundle; returns its dir. |
| `backing_attach` | `{ path: string }` | Attach a backing audio file. |
| `backing_detach` | none | Detach the backing audio file. |
| `import_url` | `{ url: string }` | Import a song from a video URL. |
| `library_scan` | none | List the bundles under the library roots. |
| `library_save` | `{ name: string }` | Save the current composition to the library. |
| `library_load` | `{ dir: string }` | Load a bundle into the composer for editing. |

Each frontend dispatches these over its own services via a `HostServices` trait
whose **exhaustive `match` is compiler-enforced**: a new command cannot be
catalogued without every frontend implementing it, so the vocabulary can't drift
behind the UI. Commands are session-stateful — an unmet precondition (e.g.
`record_save` with no session) returns a `command_failed:` error rather than
implicitly creating state. See [`specs/M8-A-host-command-tier.md`](../specs/M8-A-host-command-tier.md).

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

- [Demo scenario](DEMO-SCENARIO.md) — a guided session exercising every action,
  with the equivalent TUI keystroke per beat (agent ⇄ human parity) and an
  executable integration test
- [Development workflow](../WORKFLOW.md) — how work is tracked and delegated
- [CLAUDE.md](../CLAUDE.md) — architecture invariants and agent guide
- [`rockcraft-control` crate](../../crates/control/) — implementation of the server and protocol
- [`rockcraft-core` crate](../../crates/core/) — the pure composer engine and action vocabulary
