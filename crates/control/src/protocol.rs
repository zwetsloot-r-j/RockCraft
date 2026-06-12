use rockcraft_core::{
    action_from_name, action_help, action_names, ActionError, ActionInfo, Composer,
    ComposerSnapshot, Effect,
};
use serde::{Deserialize, Serialize};

use crate::host::{
    dispatch, host_command_from_name, host_command_help, HostCommandError, HostCommandInfo,
    HostServices,
};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    RunAction {
        id: Option<u64>,
        action: String,
        #[serde(default)]
        params: serde_json::Value,
    },
    /// Run an app-level [`HostCommand`](crate::host::HostCommand) — the
    /// host-services tier parallel to `run_action`. Dispatched by the frontend
    /// over its own services (see [`run_host_command`]), never by [`handle`].
    RunCommand {
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
            | Request::RunCommand { id, .. }
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
    /// A successful `run_command` (host-services tier). Carries the command's
    /// own JSON result (e.g. a saved bundle path or the library listing); the
    /// payload is omitted for nullary side effects.
    CommandOk {
        id: Option<u64>,
        #[serde(skip_serializing_if = "Option::is_none")]
        data: Option<serde_json::Value>,
    },
    Actions {
        id: Option<u64>,
        actions: Vec<&'static str>,
    },
    Help {
        id: Option<u64>,
        actions: Vec<ActionInfo>,
        /// The host-command tier of the catalog (app-level workflows). Omitted
        /// from the wire form only when empty, which never happens in practice.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        commands: Vec<HostCommandInfo>,
    },
    Render {
        id: Option<u64>,
        text: String,
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
            "run_command",
            "query",
            "subscribe",
            "unsubscribe",
        ],
        // Mirrors the `QueryKind` variants (PascalCase on the wire).
        queries: vec!["State", "Actions", "Help", "Render"],
        hint: r#"send {"type":"query","what":"Help"} for the full action + command catalog"#,
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

/// Build the combined `query help` response: the action catalog **and** the
/// host-command catalog, so one query discovers both protocol tiers.
///
/// Pure (no composer, no host services), so a frontend can answer `query help`
/// from any screen — even before a composition or session exists.
pub fn help_response(id: Option<u64>) -> Response {
    Response::Help {
        id,
        actions: action_help().to_vec(),
        commands: host_command_help().to_vec(),
    }
}

/// Parse and dispatch a `run_command` request against a frontend's
/// [`HostServices`], wrapping the outcome in a [`Response`].
///
/// This is the host-tier analogue of [`handle_run_action`]: where that drives
/// the pure [`Composer`], this drives the frontend's I/O services. A frontend
/// routes [`Request::RunCommand`] here (it cannot go through [`handle`], which
/// has no host services) and replies with the returned `Response`.
pub fn run_host_command(
    host: &mut dyn HostServices,
    id: Option<u64>,
    command: &str,
    params: &serde_json::Value,
) -> Response {
    match host_command_from_name(command, params) {
        Err(HostCommandError::UnknownCommand(name)) => Response::Err {
            id,
            error: format!("unknown_command: {name}"),
        },
        Err(HostCommandError::BadParams { command, detail }) => Response::Err {
            id,
            error: format!("bad_params: {command}: {detail}"),
        },
        Ok(cmd) => match dispatch(host, cmd) {
            Ok(data) => Response::CommandOk {
                id,
                // Null payloads (nullary side effects) drop off the wire.
                data: (!data.is_null()).then_some(data),
            },
            Err(error) => Response::Err {
                id,
                error: format!("command_failed: {error}"),
            },
        },
    }
}

pub fn handle(c: &mut Composer, req: Request) -> Response {
    match req {
        Request::RunAction { id, action, params } => handle_run_action(c, id, &action, &params),
        // Host commands need the frontend's services, which `handle` (composer
        // only) does not have. A frontend must route `run_command` to
        // `run_host_command` *before* falling back to `handle`; reaching here
        // means it did not, so surface that rather than silently dropping it.
        Request::RunCommand { id, .. } => Response::Err {
            id,
            error: "unavailable: run_command must be routed to host services".into(),
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
        } => help_response(id),
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
