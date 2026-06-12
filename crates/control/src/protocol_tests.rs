#[cfg(test)]
mod tests {
    use crate::protocol::{handle, handle_run_action, QueryKind, Request, Response, Topic};
    use rockcraft_core::{action_names, Composer};
    use serde_json::json;

    fn new_composer() -> Composer {
        Composer::new()
    }

    // --- Deserialisation of each Request variant ---

    #[test]
    fn deser_run_action() {
        let raw = r#"{"type":"run_action","id":1,"action":"add_note","params":{}}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(req, Request::RunAction { id: Some(1), .. }));
    }

    #[test]
    fn deser_run_action_no_params() {
        let raw = r#"{"type":"run_action","action":"cursor_left"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(req, Request::RunAction { id: None, .. }));
    }

    #[test]
    fn deser_query_state() {
        let raw = r#"{"type":"query","id":2,"what":"State"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            req,
            Request::Query {
                id: Some(2),
                what: QueryKind::State
            }
        ));
    }

    #[test]
    fn deser_query_actions() {
        let raw = r#"{"type":"query","what":"Actions"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            req,
            Request::Query {
                what: QueryKind::Actions,
                ..
            }
        ));
    }

    #[test]
    fn deser_query_render() {
        let raw = r#"{"type":"query","what":"Render"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            req,
            Request::Query {
                what: QueryKind::Render,
                ..
            }
        ));
    }

    #[test]
    fn deser_subscribe() {
        let raw = r#"{"type":"subscribe","id":5,"topic":"Events"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            req,
            Request::Subscribe {
                id: Some(5),
                topic: Topic::Events
            }
        ));
    }

    #[test]
    fn deser_unsubscribe() {
        let raw = r#"{"type":"unsubscribe","topic":"Events"}"#;
        let req: Request = serde_json::from_str(raw).unwrap();
        assert!(matches!(
            req,
            Request::Unsubscribe {
                id: None,
                topic: Topic::Events
            }
        ));
    }

    // --- handle_run_action: mutation + state ---

    #[test]
    fn run_add_note_mutates_composer() {
        let mut c = new_composer();
        let before = c.note_count();
        let resp = handle_run_action(&mut c, Some(10), "add_note", &json!({}));
        match resp {
            Response::Ok { id, state, .. } => {
                assert_eq!(id, Some(10));
                let snap = state.expect("snapshot must be present");
                assert_eq!(snap.notes.len(), before + 1);
            }
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[test]
    fn run_unknown_action_returns_err_prefix() {
        let mut c = new_composer();
        let resp = handle_run_action(&mut c, Some(99), "totally_unknown", &json!({}));
        match resp {
            Response::Err { id, error } => {
                assert_eq!(id, Some(99));
                assert!(
                    error.starts_with("unknown_action:"),
                    "expected 'unknown_action:' prefix, got: {error}"
                );
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    #[test]
    fn run_bad_params_returns_err_prefix() {
        let mut c = new_composer();
        // resize_note requires delta_steps; passing garbage triggers bad_params
        let resp = handle_run_action(&mut c, None, "resize_note", &json!({"delta_steps": "oops"}));
        match resp {
            Response::Err { error, .. } => {
                assert!(
                    error.starts_with("bad_params:"),
                    "expected 'bad_params:' prefix, got: {error}"
                );
            }
            other => panic!("expected Err, got {other:?}"),
        }
    }

    // --- handle: Query::Actions ---

    #[test]
    fn query_actions_returns_all_action_names() {
        let mut c = new_composer();
        let req = Request::Query {
            id: Some(7),
            what: QueryKind::Actions,
        };
        match handle(&mut c, req) {
            Response::Actions { id, actions } => {
                assert_eq!(id, Some(7));
                assert_eq!(actions, action_names());
            }
            other => panic!("expected Actions, got {other:?}"),
        }
    }

    #[test]
    fn query_help_returns_described_catalog() {
        let mut c = new_composer();
        let req = Request::Query {
            id: Some(9),
            what: QueryKind::Help,
        };
        match handle(&mut c, req) {
            Response::Help {
                id,
                actions,
                commands,
            } => {
                assert_eq!(id, Some(9));
                // Same coverage as the name list, but now with params + prose.
                assert_eq!(actions.len(), action_names().len());
                let names: Vec<&str> = actions.iter().map(|a| a.name).collect();
                assert_eq!(names, action_names());
                // A parametrised action carries its param schema.
                let set_cursor = actions
                    .iter()
                    .find(|a| a.name == "set_cursor")
                    .expect("set_cursor described");
                let param_names: Vec<&str> = set_cursor.params.iter().map(|p| p.name).collect();
                assert_eq!(param_names, vec!["pitch", "step"]);
                // The host-command tier is catalogued alongside the actions.
                assert_eq!(commands.len(), crate::host::host_command_names().len());
                assert!(
                    commands.iter().any(|c| c.name == "play_load"),
                    "help lists host commands too"
                );
            }
            other => panic!("expected Help, got {other:?}"),
        }
    }

    #[test]
    fn hello_banner_advertises_help_query() {
        use crate::protocol::hello;
        match hello() {
            Response::Hello {
                protocol,
                requests,
                queries,
                hint,
            } => {
                assert_eq!(protocol, "rockcraft-control/1");
                assert!(requests.contains(&"run_action"));
                assert!(requests.contains(&"run_command"));
                assert!(requests.contains(&"query"));
                assert!(queries.contains(&"Help"));
                assert!(hint.contains("Help"), "hint points at the help query");
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    // --- handle: Query::State ---

    #[test]
    fn query_state_returns_snapshot() {
        let mut c = new_composer();
        let req = Request::Query {
            id: None,
            what: QueryKind::State,
        };
        match handle(&mut c, req) {
            Response::Ok { state, .. } => {
                assert!(state.is_some());
            }
            other => panic!("expected Ok with state, got {other:?}"),
        }
    }

    // --- handle: subscribe / unsubscribe ---

    #[test]
    fn subscribe_returns_ok() {
        let mut c = new_composer();
        let req = Request::Subscribe {
            id: Some(1),
            topic: Topic::Events,
        };
        assert!(matches!(handle(&mut c, req), Response::Ok { .. }));
    }

    #[test]
    fn unsubscribe_returns_ok() {
        let mut c = new_composer();
        let req = Request::Unsubscribe {
            id: Some(2),
            topic: Topic::Events,
        };
        assert!(matches!(handle(&mut c, req), Response::Ok { .. }));
    }

    // --- Response round-trip serialisation ---

    #[test]
    fn response_ok_roundtrip() {
        let resp = Response::Ok {
            id: Some(1),
            effects: vec![],
            state: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""type":"ok""#));
    }

    #[test]
    fn response_err_roundtrip() {
        let resp = Response::Err {
            id: Some(2),
            error: "unknown_action: foo".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains(r#""type":"err""#));
        assert!(json.contains("unknown_action: foo"));
    }

    #[test]
    fn response_ok_skips_empty_effects_and_null_state() {
        let resp = Response::Ok {
            id: None,
            effects: vec![],
            state: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(!json.contains("effects"), "empty effects should be omitted");
        assert!(!json.contains("state"), "null state should be omitted");
    }
}
