# M8-A — Host-command tier: app-level workflows over agent control

> Status: implemented. Source of truth for the host-command vocabulary is the
> code, not this file — see "Single source of truth" below.

## Problem

The agent-control protocol (`crates/control`) exposes only **composer actions**:
`run_action` maps a name to a `rockcraft_core::Action` and applies it to the
pure `Composer`. That lets an agent *edit a composition* but not *drive a
session*. The app-level workflows the frontends gained in M7 —

- load a song and play it (note highway),
- run a record session (start / stop / save),
- attach / detach a backing track,
- import a song from a URL,
- scan / save / load the library,

— have **no protocol counterpart**. They are also fundamentally different from
actions: they do **I/O** (disk, MIDI device, audio output, subprocess), which
`core`'s purity invariant forbids. So they can never become `core::Action`s.

## Solution: a parallel host-command tier

Add a second request verb, `run_command`, alongside `run_action`. A *host
command* is dispatched by the **frontend over its own services**, not by `core`.

### The vocabulary (`crates/control/src/host.rs`)

`HostCommand` — a serde-tagged enum (tag = `command`, snake_case names), the
host-tier mirror of `core::Action`:

| Command | Params | Effect |
|---|---|---|
| `play_load` | `dir: string` | Load the bundle at `dir` and start playing it. |
| `play_stop` | — | Stop / tear down the active play session. |
| `record_start` | `backing: string?` | Begin a record session; optional backing under the take. |
| `record_stop` | — | Stop the session, discarding the take. |
| `record_save` | — | Save the session to a bundle (session stays open). |
| `backing_attach` | `path: string` | Attach a backing audio file. |
| `backing_detach` | — | Detach the backing audio file. |
| `import_url` | `url: string` | Run the import pipeline on a video URL. |
| `library_scan` | — | List the bundles under the library roots. |
| `library_save` | `name: string` | Save the current composition into the library. |
| `library_load` | `dir: string` | Load a bundle into the composer for editing. |

Catalog functions mirror the action catalog exactly:
`host_command_names()`, `host_command_help()` (reuses `core::ParamInfo` so the
schema is uniform), and `host_command_from_name(name, params)` for parsing.

### The compiler-enforced drift backstop

The two halves of a host command are kept in lockstep by **one exhaustive
match**:

1. `HostServices` — a trait with one method per command, implemented by each
   frontend over its own services. The methods are I/O-bearing (that is why
   they live in a frontend trait, not in `core`).
2. `dispatch(&mut dyn HostServices, HostCommand)` — routes each variant to its
   method. **The `match` is exhaustive over `HostCommand`.**

Adding a `HostCommand` variant therefore fails to compile until (a) a `dispatch`
arm and (b) a `HostServices` method exist for it — and then *every* frontend's
`impl HostServices` must implement that method. A new command **cannot** be
catalogued (and shown by `query help`) without being dispatchable by every
frontend. That is the guarantee that a command can't silently drift behind the
UI.

### Protocol shape (`crates/control/src/protocol.rs`)

- `Request::RunCommand { id?, command, params }` — the new verb.
- `Response::CommandOk { id?, data? }` — success; `data` is the command's own
  JSON result (e.g. `{ "saved": "<dir>" }`, or `{ "bundles": [...] }`); omitted
  for nullary side effects.
- Failures reuse `Response::Err` with prefixes `unknown_command:`,
  `bad_params:`, `command_failed:`.
- `Response::Help` gains a `commands: [HostCommandInfo]` field, so **one**
  `query help` lists both tiers. `help_response()` builds it with no composer
  and no host services, so a frontend answers `query help` from any screen.
- The `hello` banner lists `run_command` among the request verbs.
- `run_host_command(&mut dyn HostServices, id, name, params)` parses + dispatches
  + wraps into a `Response`. `handle` (composer-only) does **not** dispatch host
  commands — a frontend routes `run_command` to `run_host_command`.

### Frontend wiring

- **TUI** (`crates/tui/src/app.rs`): `impl HostServices for Shell`. Each method
  performs the same screen transition / service call a keypress would (open
  Play, start Record, run the import pipeline, scan/save/load library). The
  shell's `handle_remote` routes `run_command` and `query help` independent of
  the active screen.
- **Tauri** (`tauri-app/src-tauri/src/control.rs`): `impl HostServices for
  TauriHost` (holds an `AppHandle`). Each method calls the **same** backend
  service functions the IPC commands call; `library_load` re-emits the
  `snapshot` event so the webview refreshes. The applier thread routes
  `run_command` through `run_host_command`.

## Scope boundaries

- No new `core::Action`s; `core` stays pure (zero changes to `crates/core`).
- Host commands are deliberately session-stateful: a precondition that is not
  met (e.g. `record_save` with no session) returns `command_failed:` rather than
  panicking or implicitly creating state.
- The socket remains loopback-only and unauthenticated (unchanged from M4).

## Single source of truth

The live catalog is `host_command_names()` / `host_command_help()` and the
`HostCommand` enum, kept in parity by tests in `crates/control/src/host_tests.rs`
(names ↔ variants ↔ help). Prefer `query help` over any hand-maintained table —
including the one above. `docs/AGENT-CONTROL.md` and `CLAUDE.md` point here and
at `query help`; they are not authoritative on the command set.
