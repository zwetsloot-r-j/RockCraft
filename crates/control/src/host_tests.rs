//! Tests for the host-command tier: name/variant/catalog parity, parsing, and
//! the exhaustive [`dispatch`] seam.

use crate::host::*;
use serde_json::json;

/// One sample of **every** [`HostCommand`] variant. Parametrised variants use
/// minimal valid values. This is the exhaustiveness oracle the parity tests
/// cross-check against [`host_command_names`], so a forgotten variant in either
/// list surfaces as a failure.
fn all_variants() -> Vec<HostCommand> {
    vec![
        HostCommand::PlayLoad { dir: "d".into() },
        HostCommand::PlayStop,
        HostCommand::RecordStart { backing: None },
        HostCommand::RecordStop,
        HostCommand::RecordSave,
        HostCommand::BackingAttach {
            path: "b.mp3".into(),
        },
        HostCommand::BackingDetach,
        HostCommand::ImportUrl {
            url: "http://x".into(),
        },
        HostCommand::LibraryScan,
        HostCommand::LibrarySave { name: "n".into() },
        HostCommand::LibraryLoad { dir: "d".into() },
    ]
}

/// The serde tag every variant serialises under — the wire `command` name.
fn wire_name(cmd: &HostCommand) -> String {
    serde_json::to_value(cmd).unwrap()["command"]
        .as_str()
        .unwrap()
        .to_string()
}

#[test]
fn names_match_variants() {
    let mut from_variants: Vec<String> = all_variants().iter().map(wire_name).collect();
    from_variants.sort();
    let mut names: Vec<String> = host_command_names().iter().map(|s| s.to_string()).collect();
    names.sort();
    assert_eq!(
        from_variants, names,
        "host_command_names() must list exactly the HostCommand variant tags"
    );
}

#[test]
fn help_covers_exactly_the_names() {
    let mut help_names: Vec<&str> = host_command_help().iter().map(|h| h.name).collect();
    help_names.sort_unstable();
    let mut names = host_command_names().to_vec();
    names.sort_unstable();
    assert_eq!(
        help_names, names,
        "host_command_help() must cover exactly host_command_names()"
    );
}

#[test]
fn every_name_round_trips_through_from_name() {
    // Minimal valid params per command so the parse succeeds for all of them.
    let params = |name: &str| -> serde_json::Value {
        match name {
            "play_load" | "library_load" => json!({ "dir": "d" }),
            "backing_attach" => json!({ "path": "b.mp3" }),
            "import_url" => json!({ "url": "http://x" }),
            "library_save" => json!({ "name": "n" }),
            _ => json!({}),
        }
    };
    for name in host_command_names() {
        let parsed = host_command_from_name(name, &params(name))
            .unwrap_or_else(|e| panic!("`{name}` should parse: {e}"));
        assert_eq!(&wire_name(&parsed), name, "round-trip name mismatch");
    }
}

#[test]
fn unknown_command_is_rejected() {
    let err = host_command_from_name("frobnicate", &json!({})).unwrap_err();
    assert_eq!(err, HostCommandError::UnknownCommand("frobnicate".into()));
}

#[test]
fn missing_required_param_is_bad_params() {
    let err = host_command_from_name("play_load", &json!({})).unwrap_err();
    assert!(
        matches!(err, HostCommandError::BadParams { ref command, .. } if command == "play_load"),
        "got {err:?}"
    );
}

#[test]
fn record_start_backing_defaults_to_none() {
    let cmd = host_command_from_name("record_start", &json!({})).unwrap();
    assert_eq!(cmd, HostCommand::RecordStart { backing: None });
    let cmd = host_command_from_name("record_start", &json!({ "backing": "x.wav" })).unwrap();
    assert_eq!(
        cmd,
        HostCommand::RecordStart {
            backing: Some("x.wav".into())
        }
    );
}

/// A [`HostServices`] that records which method ran and echoes its arguments,
/// so the [`dispatch`] routing can be asserted without a real frontend.
#[derive(Default)]
struct SpyHost {
    last: Option<String>,
}

impl SpyHost {
    fn note(&mut self, what: impl Into<String>) -> HostResult {
        let what = what.into();
        self.last = Some(what.clone());
        Ok(json!({ "ran": what }))
    }
}

impl HostServices for SpyHost {
    fn play_load(&mut self, dir: &str) -> HostResult {
        self.note(format!("play_load:{dir}"))
    }
    fn play_stop(&mut self) -> HostResult {
        self.note("play_stop")
    }
    fn record_start(&mut self, backing: Option<&str>) -> HostResult {
        self.note(format!("record_start:{}", backing.unwrap_or("-")))
    }
    fn record_stop(&mut self) -> HostResult {
        self.note("record_stop")
    }
    fn record_save(&mut self) -> HostResult {
        self.note("record_save")
    }
    fn backing_attach(&mut self, path: &str) -> HostResult {
        self.note(format!("backing_attach:{path}"))
    }
    fn backing_detach(&mut self) -> HostResult {
        self.note("backing_detach")
    }
    fn import_url(&mut self, url: &str) -> HostResult {
        self.note(format!("import_url:{url}"))
    }
    fn library_scan(&mut self) -> HostResult {
        self.note("library_scan")
    }
    fn library_save(&mut self, name: &str) -> HostResult {
        self.note(format!("library_save:{name}"))
    }
    fn library_load(&mut self, dir: &str) -> HostResult {
        self.note(format!("library_load:{dir}"))
    }
}

#[test]
fn dispatch_routes_each_variant_to_its_method() {
    let cases = [
        (
            HostCommand::PlayLoad { dir: "song".into() },
            "play_load:song",
        ),
        (HostCommand::PlayStop, "play_stop"),
        (
            HostCommand::RecordStart {
                backing: Some("b".into()),
            },
            "record_start:b",
        ),
        (HostCommand::RecordStop, "record_stop"),
        (HostCommand::RecordSave, "record_save"),
        (
            HostCommand::BackingAttach { path: "p".into() },
            "backing_attach:p",
        ),
        (HostCommand::BackingDetach, "backing_detach"),
        (HostCommand::ImportUrl { url: "u".into() }, "import_url:u"),
        (HostCommand::LibraryScan, "library_scan"),
        (
            HostCommand::LibrarySave { name: "n".into() },
            "library_save:n",
        ),
        (
            HostCommand::LibraryLoad { dir: "d".into() },
            "library_load:d",
        ),
    ];
    for (cmd, expected) in cases {
        let mut host = SpyHost::default();
        let out = dispatch(&mut host, cmd).expect("spy never errors");
        assert_eq!(host.last.as_deref(), Some(expected), "dispatch routing");
        assert_eq!(out["ran"].as_str(), Some(expected), "returned data");
    }
}
