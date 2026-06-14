use rockcraft_core::{
    action_from_name, action_help, action_names, ActionError, ActionInfo, Composer,
    ComposerSnapshot, Effect,
};
use serde::{Deserialize, Serialize};

use crate::host::{host_command_from_name, host_help, HostCommandInfo, HostError, HostServices};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    RunAction {
        id: Option<u64>,
        action: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    /// Run an app-level [`HostCommand`](crate::host::HostCommand) — the I/O tier
    /// dispatched by the frontend's [`HostServices`], never a `core::Action`.
    /// Reaches a [`HostServices`] impl only via [`handle_with_host`]; the
    /// composer-only [`handle`] rejects it.
    RunHostCommand {
        id: Option<u64>,
        command: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    Query {
        id: Option<u64>,
        what: QueryKind,
    },
    Subscribe {
        id: Option<u64>,
        topic: Topic,
    },
    Unsubscribe {
        id: Option<u64>,
        topic: Topic,
    },
}

impl Request {
    /// The correlation id the client attached to this request, if any. Used to
    /// echo the id back on responses synthesised outside [`handle`] (e.g. a
    /// channel-closed error from the socket task).
    pub fn id(&self) -> Option<u64> {
        match self {
            Request::RunAction { id, .. }
            | Request::RunHostCommand { id, .. }
            | Request::Query { id, .. }
            | Request::Subscribe { id, .. }
            | Request::Unsubscribe { id, .. } => *id,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Response {
    Ok {
        id: Option<u64>,
        #[serde(skip_serializing_if = "Vec::is_empty")]
        effects: Vec<Effect>,
        #[serde(skip_serializing_if = "Option::is_none")]
        state: Option<ComposerSnapshot>,
    },
    Err {
        id: Option<u64>,
        error: String,
    },
    Actions {
        id: Option<u64>,
        actions: Vec<&'static str>,
    },
    /// The self-describing catalog for `query help`. Carries **both** tiers:
    /// `actions` (`core::action_help`, the pure composer ops) and
    /// `host_commands` (`crate::host::host_help`, the app-level I/O ops). An
    /// agent discovers the whole surface from one query.
    Help {
        id: Option<u64>,
        actions: Vec<ActionInfo>,
        host_commands: Vec<HostCommandInfo>,
    },
    Render {
        id: Option<u64>,
        text: String,
    },
    /// The result of a successful [`Request::RunHostCommand`]: the JSON value the
    /// frontend's [`HostServices`] returned (status / dirty / info, or `null`).
    /// Distinct from `Ok` so a host result never carries a composer snapshot.
    HostResult {
        id: Option<u64>,
        value: serde_json::Value,
    },
    /// Unsolicited banner sent once, right after the WebSocket handshake, before
    /// the client sends anything. It names the protocol verbs and points at
    /// `query help`, so even an agent that never read the docs can bootstrap.
    Hello {
        protocol: &'static str,
        requests: Vec<&'static str>,
        queries: Vec<&'static str>,
        hint: &'static str,
    },
    Event {
        topic: Topic,
        event: serde_json::Value,
    },
}

/// The connection banner (a [`Response::Hello`]) every server sends on connect.
///
/// Kept in one place so both [`crate::server::ControlServer`] and
/// [`crate::command::CommandServer`] greet clients identically.
pub fn hello() -> Response {
    Response::Hello {
        protocol: "rockcraft-control/1",
        requests: vec![
            "run_action",
            "run_host_command",
            "query",
            "subscribe",
            "unsubscribe",
        ],
        // Mirrors the `QueryKind` variants (PascalCase on the wire).
        queries: vec!["State", "Actions", "Help", "Render"],
        hint: r#"send {"type":"query","what":"Help"} for the full action + host-command catalog"#,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum QueryKind {
    State,
    Actions,
    Help,
    Render,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum Topic {
    Events,
}

pub fn handle_run_action(
    c: &mut Composer,
    id: Option<u64>,
    action: &str,
    params: &serde_json::Value,
) -> Response {
    match action_from_name(action, params) {
        Err(ActionError::UnknownAction(name)) => Response::Err {
            id,
            error: format!("unknown_action: {name}"),
        },
        Err(ActionError::BadParams {
            action: act,
            detail,
        }) => Response::Err {
            id,
            error: format!("bad_params: {act}: {detail}"),
        },
        Ok(a) => match c.apply(a) {
            Ok(effects) => Response::Ok {
                id,
                effects,
                state: Some(c.snapshot()),
            },
            Err(ActionError::UnknownAction(name)) => Response::Err {
                id,
                error: format!("unknown_action: {name}"),
            },
            Err(ActionError::BadParams {
                action: act,
                detail,
            }) => Response::Err {
                id,
                error: format!("bad_params: {act}: {detail}"),
            },
        },
    }
}

/// Dispatch one [`Request::RunHostCommand`] against a frontend's [`HostServices`].
///
/// Parses `command`/`params` into a [`HostCommand`](crate::host::HostCommand)
/// via [`host_command_from_name`] (same name/serde-tag contract and
/// `unknown_action:` / `bad_params:` error prefixes as the action tier), then
/// hands it to `host`. A successful dispatch returns the host's JSON value under
/// `Response::Event { topic: Events, .. }`-free `Ok`-style framing — concretely
/// a [`Response::HostResult`] carrying the value; a failed dispatch maps to
/// `Response::Err`.
pub fn handle_run_host_command(
    host: &mut dyn HostServices,
    id: Option<u64>,
    command: &str,
    params: &serde_json::Value,
) -> Response {
    match host_command_from_name(command, params) {
        Err(ActionError::UnknownAction(name)) => Response::Err {
            id,
            error: format!("unknown_action: {name}"),
        },
        Err(ActionError::BadParams {
            action: act,
            detail,
        }) => Response::Err {
            id,
            error: format!("bad_params: {act}: {detail}"),
        },
        Ok(cmd) => match host.dispatch(cmd) {
            Ok(value) => Response::HostResult { id, value },
            Err(HostError::Unsupported(name)) => Response::Err {
                id,
                error: format!("unsupported: {name}"),
            },
            Err(HostError::Failed { command, detail }) => Response::Err {
                id,
                error: format!("host_failed: {command}: {detail}"),
            },
        },
    }
}

/// Handle a request with **no** host services available.
///
/// The composer-owning [`crate::ControlServer`] uses this: it owns no
/// play/record/etc. state, so a [`Request::RunHostCommand`] is rejected with an
/// `unavailable:` error. All other requests behave exactly as before. The
/// channel-routed [`crate::CommandServer`] frontends use [`handle_with_host`]
/// instead, threading their `HostServices`.
pub fn handle(c: &mut Composer, req: Request) -> Response {
    handle_with_host(c, None, req)
}

/// Handle a request, dispatching [`Request::RunHostCommand`] to `host` when one
/// is supplied. Composer actions/queries are handled exactly as [`handle`].
///
/// Frontends pass `Some(&mut their_host_services)` so app-level commands reach
/// their own play/record/import/library/audio helpers; `None` rejects host
/// commands with `unavailable: …`.
pub fn handle_with_host(
    c: &mut Composer,
    host: Option<&mut dyn HostServices>,
    req: Request,
) -> Response {
    match req {
        Request::RunAction { id, action, params } => handle_run_action(c, id, &action, &params),
        Request::RunHostCommand {
            id,
            command,
            params,
        } => match host {
            Some(host) => handle_run_host_command(host, id, &command, &params),
            None => Response::Err {
                id,
                error: "unavailable: host commands not supported on this server".into(),
            },
        },
        Request::Query {
            id,
            what: QueryKind::Actions,
        } => Response::Actions {
            id,
            actions: action_names().to_vec(),
        },
        Request::Query {
            id,
            what: QueryKind::Help,
        } => Response::Help {
            id,
            actions: action_help().to_vec(),
            host_commands: host_help().to_vec(),
        },
        Request::Query {
            id,
            what: QueryKind::State,
        } => Response::Ok {
            id,
            effects: vec![],
            state: Some(c.snapshot()),
        },
        Request::Query {
            id,
            what: QueryKind::Render,
        } => Response::Render {
            id,
            text: String::new(),
        },
        Request::Subscribe { id, .. } => Response::Ok {
            id,
            effects: vec![],
            state: None,
        },
        Request::Unsubscribe { id, .. } => Response::Ok {
            id,
            effects: vec![],
            state: None,
        },
    }
}
