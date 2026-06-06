# M4-D — control: JSON protocol types + generic `run_action`

> Milestone: M4 — Agent Interface · Issue: #88 · Suggested tier: sonnet
> Branch: `claude/m4-control-protocol`

## Goal

Create the new `crates/control` workspace member and define the request/response
protocol an external client (an AI agent, a test harness) uses to drive a
running RockCraft. This task is **pure, socket-free**: just the serde types plus
the generic `run_action` → `core::Action` mapping, fully unit-tested. The
WebSocket transport is M4-E.

## Context

- New crate `crates/control` → `rockcraft-control`; add to the workspace
  `members`. Depends on `rockcraft-core` (for `Action`/`Effect`/`ComposerSnapshot`
  and `action_from_name`/`action_names`) and `serde`/`serde_json`.
- The protocol is line/message-oriented JSON (one JSON value per WebSocket text
  message). Keep it generic so future `Action`s need no protocol change — a
  `run_action` carries the action name + params straight to M4-A's
  `action_from_name`.

## What to do

```rust
// crates/control/src/protocol.rs
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    RunAction { id: Option<u64>, action: String,
                #[serde(default)] params: serde_json::Value },
    Query     { id: Option<u64>, what: QueryKind },      // state | actions | render
    Subscribe { id: Option<u64>, topic: Topic },         // events
    Unsubscribe { id: Option<u64>, topic: Topic },
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok    { id: Option<u64>, #[serde(skip_serializing_if="Vec::is_empty")]
            effects: Vec<Effect>, #[serde(skip_serializing_if="Option::is_none")]
            state: Option<ComposerSnapshot> },
    Err   { id: Option<u64>, error: String },
    Actions { id: Option<u64>, actions: Vec<&'static str> },  // for `query actions`
    Render  { id: Option<u64>, text: String },                // M4-G fills this
    Event   { topic: Topic, event: serde_json::Value },       // pushed, no id
}

#[derive(Debug, Clone, Serialize, Deserialize)] pub enum QueryKind { State, Actions, Render }
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)] pub enum Topic { Events }

/// Apply a parsed `Request::RunAction` against a `Composer`, returning the
/// `Response`. Pure: takes `&mut Composer`, no I/O. M4-E calls this per message.
pub fn handle_run_action(c: &mut Composer, id: Option<u64>,
                         action: &str, params: &serde_json::Value) -> Response;
```

- `handle_run_action` calls `core::action_from_name(action, params)`; on success
  `composer.apply(..)` and return `Ok { effects, state: Some(snapshot) }`; on
  `ActionError` return `Err { error }` with a stable, greppable message
  (e.g. `unknown_action: foo`, `bad_params: resize_note: ...`).
- Provide `fn handle(c: &mut Composer, req: Request) -> Response` dispatching all
  variants (`Query::Actions` → `action_names()`; `Query::State` → snapshot;
  `Query::Render` → empty/placeholder until M4-G; subscribe/unsubscribe return
  `Ok`). Returning a snapshot on every `RunAction` is deliberate: one round-trip
  edits *and* reads back, so an agent verifies without a second call.

## Tests (control crate, no socket)

- Deserialise representative request JSON for each variant.
- `handle_run_action` with `"add_note"` mutates the composer (note_count +1) and
  returns `Ok` with a non-empty `state`; unknown action → `Err` with the
  `unknown_action:` prefix; bad params → `Err` with `bad_params:`.
- `Query::Actions` returns exactly `core::action_names()`.
- Response JSON shape round-trips (serialise → parse) for `Ok`/`Err`.

## Scope boundaries (do NOT)

- No sockets / tokio / async here (M4-E). No file ops (load/save) yet.
- Do not reach into the TUI. Do not change `core` signatures.
- Only deps: `rockcraft-core`, `serde`, `serde_json`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green
- [ ] PR against `main` from `claude/m4-control-protocol`, `Closes #88`
