use rockcraft_core::{
    action_from_name, action_help, action_names, ActionError, ActionInfo, Composer,
    ComposerSnapshot, Effect,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Request {
    RunAction {
        id: Option<u64>,
        action: String,
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
    Help {
        id: Option<u64>,
        actions: Vec<ActionInfo>,
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
        requests: vec!["run_action", "query", "subscribe", "unsubscribe"],
        // Mirrors the `QueryKind` variants (PascalCase on the wire).
        queries: vec!["State", "Actions", "Help", "Render"],
        hint: r#"send {"type":"query","what":"Help"} for the full action catalog"#,
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

pub fn handle(c: &mut Composer, req: Request) -> Response {
    match req {
        Request::RunAction { id, action, params } => handle_run_action(c, id, &action, &params),
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
