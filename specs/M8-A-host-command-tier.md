# M8-A — control: a host-command tier for app-level workflows (play/record/import/library/backing)

> Milestone: M8 — Agent Interface v2 · Issue: #194 · Suggested tier: opus
> Branch: `claude/control-host-commands`
> Depends on: M7-tauri-O (#193, control socket on Tauri)

## Goal

Make the app-level workflows that the frontends gained in M7 — load a song to
play, run a record session, attach/detach a backing track, import from a URL,
scan/save/load the library — reachable over the **agent-control protocol**, so an
agent can drive a whole session end-to-end, not just edit the composition.

These operations are **not** `core::Action`s and must never become them: they do
I/O (disk, MIDI device, audio, subprocess), which `core` forbids
(`CLAUDE.md` — "`core` stays pure"). So they cannot ride the existing
`action_from_name` path. This spec adds a parallel, **host-dispatched** command
tier alongside actions, with the same self-describing `query help` discovery and
the same drift-proofing — but with dispatch implemented by each frontend over
its own services.

## Why this design (read before coding)

The composer-action surface is already drift-proof and **auto-wired to every
frontend**: a new `core::Action` shows up on the TUI socket, the Tauri socket
(after M7-tauri-O), and Tauri IPC for free, because all three dispatch
generically through `core::action_from_name` / `core::action_help`. App-level
commands get **none** of that today and, being I/O, can't live in `core`.

The goal of this tier is to give app-level commands the *same two properties*:
1. **One catalog** — `query help` lists them with params + prose, like actions,
   so an agent discovers them live with no hand-maintained doc table.
2. **Can't-forget wiring** — adding a command forces every frontend to handle it
   (compiler-enforced), so the API can't silently drift behind the UI.

We get (2) by routing dispatch through a **trait with an exhaustive match**, so
the compiler is the reviewer (per `CLAUDE.md`).

## Context

- App-level commands today (all `#[tauri::command]`, no protocol counterpart):
  - **Library/bundles:** `scan_library`, `save_bundle`, `load_bundle`,
    `query_dirty` (`tauri-app/src-tauri/src/{library.rs,state.rs}`)
  - **Play:** `play_load`, `play_set_wait`, `play_toggle_hear_song`, `play_finish`
    (`src/play.rs`)
  - **Record:** `record_start`, `record_stop`, `record_save`, `record_status`
    (`src/record.rs`)
  - **Backing/audio:** `attach_backing`, `detach_backing`, `audio_status`
    (`src/audio.rs`)
  - **Import:** `import_url_available`, `import_start` (`src/import.rs`)
  - **MIDI device:** `midi_status`, `mock_key` (`src/midi.rs`)
- The TUI has the equivalent behaviour in `crates/tui/src/{record.rs,play.rs}` +
  its bundle/backing handling.
- Protocol: `crates/control/src/protocol.rs` (`Request`, `QueryKind`, `handle`),
  `command.rs` (`RemoteCommand` seam). Composer actions:
  `crates/core/src/action.rs` (`Action`, `action_from_name`, `action_help`,
  `ActionInfo`/`ParamInfo`, and the parity tests that keep the lists in lockstep).

## What to do

### 1. Define the host-command catalog (in `crates/control`, NOT `core`)

Mirror the `core::action.rs` shape, but for host commands. `control` already
depends on `core` and is depended on by both frontends, and it carries no I/O
itself — only the vocabulary:

```rust
// crates/control/src/host.rs
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "command", rename_all = "snake_case")]
pub enum HostCommand {
    // library
    ScanLibrary,
    SaveBundle { dest: SaveDest },
    LoadBundle { dir: String },
    QueryDirty,
    // play
    PlayLoad { dir: String },
    PlaySetWait { on: bool },
    // record
    RecordStart { /* … */ },
    RecordStop,
    RecordSave,
    // backing
    AttachBacking { path: String },
    DetachBacking,
    // import
    ImportStart { url: String, /* … */ },
    // status / device
    AudioStatus, MidiStatus, RecordStatus,
    // …exact set = the table above; settle naming with the frontend authors
}

impl HostCommand { pub fn name(&self) -> &'static str { /* exhaustive match */ } }

pub struct HostCommandInfo { pub name: &'static str, pub params: &'static [ParamInfo], pub description: &'static str }
pub fn host_help() -> &'static [HostCommandInfo];        // mirrors action_help()
pub fn host_command_from_name(name: &str, params: &Value) -> Result<HostCommand, ActionError>;
```

Reuse `core`'s `ParamInfo` / `ActionError`. Add the **same parity tests** that
`action.rs` has: `name()`/serde-tag parity, `host_help()` covers exactly the
catalog, every help param-set dispatches, names unique/non-empty.

### 2. Dispatch via a host-implemented trait (the compiler-enforced seam)

```rust
// crates/control/src/host.rs
pub trait HostServices {
    /// Apply one host command against the frontend's own services.
    /// Returns a JSON value for `query`-style commands (status/dirty), or unit.
    fn dispatch(&mut self, cmd: HostCommand) -> Result<serde_json::Value, HostError>;
}
```

Each frontend implements `dispatch` with a **single exhaustive `match` on
`HostCommand`** — so adding a variant fails to compile until every frontend
handles it. Tauri's impl calls the existing `play.rs`/`record.rs`/… helpers;
the TUI's impl calls its equivalents. (Where a frontend genuinely can't support a
command, it returns `HostError::Unsupported` — an explicit arm, still
compiler-checked.)

### 3. Thread it through the protocol

- Add `Request::RunHostCommand { id, command, params }` and a
  `QueryKind`/response addition so `query help` returns **both** `actions`
  (`action_help`) and `host_commands` (`host_help`). Update the `hello` banner
  verbs/queries list and the `handle` signature so host commands reach the
  frontend's `HostServices` (extend the `RemoteCommand` seam — the app loop
  already owns play/record/etc. state, so this is where dispatch lands).
- Keep `run_action` untouched and separate; the two tiers coexist.

### 4. Keep the API in sync going forward (drift backstop)

- Add a short **"Agent-control API: single source of truth"** subsection to
  `CLAUDE.md` stating: composer ops are `core::Action`s (auto-wired to all
  frontends via `action_from_name`/`action_help`; the `action.rs` parity tests
  enforce the catalog); app-level ops are `control::HostCommand`s (compiler
  forces every `HostServices` impl to handle new variants; the `host.rs` parity
  tests enforce the catalog). The rule: a new user-facing capability gets an
  `Action` (if pure) or a `HostCommand` (if it does I/O) — never a one-off IPC
  command with no protocol counterpart.
- Update `docs/AGENT-CONTROL.md` to document the host-command tier and the
  extended `query help` shape.

## Tests

- `crates/control` parity tests for `HostCommand` mirroring the `action.rs`
  battery (name/tag parity, `host_help` coverage, dispatch-from-help-params,
  uniqueness).
- A fake `HostServices` in `control` tests proving `RunHostCommand` round-trips
  through `handle` and returns the dispatcher's value/error.
- Tauri: a headless test driving a representative host command (e.g.
  `LoadBundle` then `QueryDirty`) over the socket against a real `AppState` +
  the frontend `HostServices` impl, asserting the same effect as the IPC path.
- A compile-fail-style guard is unnecessary (the exhaustive match *is* the
  guard), but add one test per frontend asserting `dispatch` handles a sample of
  each command without panicking.

## Scope boundaries (do NOT)

- Do **not** add any I/O, device, audio, or file-format code to `crates/core`,
  and do **not** turn any host command into a `core::Action`. The purity
  invariant is the whole reason this tier exists.
- Do **not** duplicate the play/record/import/library logic — `HostServices`
  impls call the **existing** frontend helpers (`play.rs`, `record.rs`,
  `library.rs`, `audio.rs`, `import.rs`); this spec is the protocol seam, not a
  rewrite of those workflows.
- Do **not** break or change the existing `run_action` action tier or the IPC
  commands; host commands are additive.
- Settle the exact command set/param shapes against the current frontend
  helpers; do not invent capabilities the app doesn't have.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `query help` over the socket returns both the action and host-command
      catalogs; an agent can `LoadBundle` + start play/record over the socket
- [ ] `CLAUDE.md` + `docs/AGENT-CONTROL.md` updated per §4
- [ ] PR opened against `main` from the branch above, `Closes #194`
