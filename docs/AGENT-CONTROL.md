# Agent Control Protocol

> **Status:** Stable (M4) — see [CLAUDE.md](../CLAUDE.md) for architecture invariants.

This document describes the WebSocket control interface that lets an AI agent (or a human) connect to a running RockCraft instance and drive edits programmatically. The interface is built on the `rockcraft-control` crate and exposes the full action vocabulary from `core::action_names()`.

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
the same `snapshot` event the IPC path does).

```bash
# Same protocol, the desktop host instead of the TUI:
cargo run --bin rockcraft-tauri -- --control
```

> Scope note: the control socket speaks the **composer-action** protocol on both
> hosts. App-level workflows (load-to-play, run a record session, import,
> library, backing) are the separate **M8-A host-command tier**, not actions.

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
  "requests": ["run_action", "run_host_command", "query", "subscribe", "unsubscribe"],
  "queries": ["State", "Actions", "Help", "Render"],
  "hint": "send {\"type\":\"query\",\"what\":\"Help\"} for the full action + host-command catalog"
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
| `run_action` | Execute a pure composer action (`core::Action`) | [Below](#run_action) |
| `run_host_command` | Execute an app-level workflow (`control::HostCommand`) | [Below](#run_host_command) |
| `query` | Query state, actions, help, or render | [Below](#query) |
| `subscribe` | Subscribe to events | [Below](#subscribe) |
| `unsubscribe` | Unsubscribe from events | [Below](#unsubscribe) |

The control surface has **two tiers**. `run_action` drives pure composer edits
(`core::Action`); `run_host_command` drives app-level workflows that do I/O —
load/save/scan a bundle, play/record, attach a backing track, import from a URL
(`control::HostCommand`). Both are discoverable from one `query help` (which
returns `actions` **and** `host_commands`). See the
[host-command vocabulary](#host-command-vocabulary).

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

##### `run_host_command`

Executes a single app-level command from the
[host-command vocabulary](#host-command-vocabulary). Same name/params shape as
`run_action`, but the `command` field names a `control::HostCommand` and the
frontend dispatches it through its own services (disk / device / audio /
subprocess).

**Request:**
```json
{
  "type": "run_host_command",
  "id": 1,
  "command": "load_bundle",
  "params": { "dir": "recordings/take-2026-06-15" }
}
```

**Response (success):** a `host_result` carrying the command's JSON value
(status / dirty / info, or `null`). Unlike `run_action` it never carries a
composer snapshot — query `state` afterward if you need one (the Tauri host also
emits a `snapshot` event after a bundle load):
```json
{
  "type": "host_result",
  "id": 1,
  "value": { ... new composer snapshot for load_bundle ... }
}
```

**Response (error):**
```json
{ "type": "err", "id": 1, "error": "host_failed: load_bundle: read failed: ..." }
```

Error prefixes: `unknown_action:` (no such command), `bad_params:` (missing or
mistyped field), `unsupported:` (this frontend cannot perform it — e.g. the TUI
returns this for `record_start`), `host_failed:` (the command ran and failed).
On the composer-only server (`ControlServer`) a host command returns
`unavailable: …` since it owns no app-level state.

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
  "host_commands": [
    {
      "name": "load_bundle",
      "params": [ { "name": "dir", "type": "String" } ],
      "description": "Load a bundle directory into the composer, replacing its timeline. Returns the new snapshot."
    },
    {
      "name": "play_set_wait",
      "params": [ { "name": "on", "type": "bool" } ],
      "description": "Arm (true) or disarm (false) note-by-note wait mode for the play session."
    }
  ]
}
```

`help` is the structured superset of `actions` **and** `host_commands`: it lists
**every** action (from `core::action_help()`) and **every** host command (from
`control::host_help()`) with its parameter schema (`name` + Rust scalar `type`)
and a one-line description. Query it once on connect to discover the full call
shape of both tiers — no hand-maintained table to consult.

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

### Host-command vocabulary

App-level workflows are the second tier — `run_host_command`, not `run_action`.
They do I/O, so they live in `control::HostCommand` (not `core::Action`) and are
dispatched by each frontend's `HostServices`. The full, drift-proof catalog is
in `query help` under `host_commands` (from `control::host_help()`); the table
below is an at-a-glance convenience only.

| Command | Params | Description |
|---------|--------|-------------|
| `scan_library` | none | Scan the default library roots; returns the bundle list |
| `save_bundle` | `{ dest: SaveDest }` | Save the timeline; `dest` is `{kind:"quick_save"}` or `{kind:"library",name:"…"}`. Returns the bundle dir |
| `load_bundle` | `{ dir: String }` | Load a bundle into the composer. Returns the new snapshot |
| `query_dirty` | none | Whether the timeline has unsaved changes |
| `split_bundle` | `{ segments: [{ start_us, end_us, name }] }` | Slice the loaded piece into the kept parts, each a new library bundle (subset MIDI + copied media + derived offsets, `origin=Edited`). Discarded parts are omitted (= trimming). Returns the created bundle dirs; the source is untouched |
| `play_load` | `{ dir: String }` | Load a bundle as a play session. Returns play info |
| `play_set_wait` | `{ on: bool }` | Arm/disarm note-by-note wait mode |
| `play_toggle_hear_song` | none | Toggle the audible song synth |
| `play_toggle_pause` | none | Pause/resume the active play session, freezing/thawing the clock + backing at the current position. No-op with no active session |
| `play_finish` | none | Finish the play session; returns the score summary |
| `record_start` | `{ backing: String? }` | Start a record session, optionally over a backing file |
| `record_stop` | none | Stop recording without saving |
| `record_save` | none | Save the session as a bundle. Returns the dir |
| `attach_backing` | `{ path: String }` | Attach a backing audio file |
| `detach_backing` | none | Detach the backing audio file |
| `attach_video` | `{ path: String, offset_us: i64 }` | Attach a background video ("the movie"); persisted into the bundle on save. Returns the `VideoRef` |
| `set_video_offset` | `{ offset_us: i64 }` | Re-align the attached video. Returns the `VideoRef` |
| `detach_video` | none | Detach the background video |
| `query_video` | none | The attached background video, or `null` |
| `import_start` | `{ url: String }` | Start importing from a URL |
| `import_score` | `{ path: String }` | Start importing a local score file (MusicXML/`.xml`/`.mxl`/`.abc`/`.krn`). The notated tempo, metre and key seed the new bundle's grid |
| `import_score` (scan) | `{ path: String }` | The same command with a `.pdf`/`.png`/`.jpg`/`.jpeg`/`.tif`/`.tiff`/`.bmp` path runs optical music recognition first. Lossy: notes carry a derived `confidence`, and the import log emits `omr: imported N notes, M flagged …`. Needs an OMR engine installed — see [`IMPORT.md`](IMPORT.md#scanned-sheet-music-omr-m13-b) |
| `audio_status` / `midi_status` / `record_status` | none | Read-only status snapshots |
| `app_quit` | none | Shut the app down gracefully (exit 0); the socket closes as the process exits |

Not every frontend supports every command: the TUI's record/import/backing
flows are interactive screen state machines, so it returns `unsupported:` for
those (it wires `scan_library`, `query_dirty`, `play_load`). The Tauri desktop
host backs the full set. Always discover the live set with `query help`.

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
- [Backing-movie scenario](BACKING-MOVIE-SCENARIO.md) — an end-to-end host-command
  session against the Tauri app: start it, author a song with a movie backing,
  save, play back, then quit — with an executable driver that does it all
- [Development workflow](../WORKFLOW.md) — how work is tracked and delegated
- [CLAUDE.md](../CLAUDE.md) — architecture invariants and agent guide
- [`rockcraft-control` crate](../../crates/control/) — implementation of the server and protocol
- [`rockcraft-core` crate](../../crates/core/) — the pure composer engine and action vocabulary
