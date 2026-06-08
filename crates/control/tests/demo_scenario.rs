//! Executable companion to `docs/DEMO-SCENARIO.md`.
//!
//! This drives the *entire* action vocabulary over a real loopback WebSocket,
//! exactly as the demo screenplay narrates it, and asserts the resulting state
//! snapshot at each beat. It serves three purposes at once:
//!
//! 1. **Parity proof** — every beat here is the agent (`run_action`) side of a
//!    doc beat whose other column is the equivalent human TUI keystroke. If the
//!    agent path and the human path ever diverge, the doc table and this test go
//!    out of step and a reviewer notices.
//! 2. **Integration test** — it exercises the protocol end to end (server bind,
//!    websocket handshake, JSON frames, shared `Composer`).
//! 3. **Regression guard** — adding/renaming an action without updating the
//!    discovery surface trips the Act 0 assertions (help/actions vs
//!    `action_names()`).
//!
//! Beat numbers and ordering mirror `docs/DEMO-SCENARIO.md`; keep them in sync.

use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use rockcraft_control::ControlServer;
use rockcraft_core::{action_names, Composer};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// Grid step at the default 120 BPM / sixteenth subdivision: a quarter note is
/// 500 000 µs, a sixteenth is a quarter of that.
const STEP_US: u64 = 125_000;

async fn spawn_server() -> (String, tokio::task::JoinHandle<()>) {
    let composer = Arc::new(Mutex::new(Composer::new()));
    let server = ControlServer::bind("127.0.0.1:0", composer)
        .await
        .expect("bind loopback");
    let url = format!("ws://{}", server.local_addr());
    let handle = tokio::spawn(async move {
        let _ = server.serve().await;
    });
    (url, handle)
}

async fn connect(url: &str) -> Ws {
    let (ws, _resp) = tokio_tungstenite::connect_async(url)
        .await
        .expect("client connects");
    ws
}

/// Read the next text frame, parsed (raw — does not skip unsolicited frames).
async fn recv(ws: &mut Ws) -> Value {
    loop {
        match ws.next().await.expect("a frame").expect("no ws error") {
            Message::Text(t) => return serde_json::from_str(&t).expect("frame is json"),
            _ => continue, // ignore control frames
        }
    }
}

/// Send one JSON request and return its response, skipping unsolicited server
/// frames (the connect banner and any async events) so responses stay 1:1.
async fn send(ws: &mut Ws, req: Value) -> Value {
    ws.send(Message::Text(req.to_string())).await.expect("send");
    loop {
        let v = recv(ws).await;
        if v["type"] == "hello" || v["type"] == "event" {
            continue;
        }
        return v;
    }
}

/// Run an action and assert it succeeded; returns the full reply.
async fn run(ws: &mut Ws, action: &str, params: Value) -> Value {
    let reply = send(
        ws,
        json!({ "type": "run_action", "action": action, "params": params }),
    )
    .await;
    assert_eq!(
        reply["type"], "ok",
        "action `{action}` should succeed, got {reply}"
    );
    reply
}

// ── snapshot accessors ──────────────────────────────────────────────────────

fn notes(reply: &Value) -> &Vec<Value> {
    reply["state"]["notes"].as_array().expect("notes array")
}
fn note_count(reply: &Value) -> usize {
    notes(reply).len()
}
fn find_pitch(reply: &Value, pitch: u64) -> Option<&Value> {
    notes(reply).iter().find(|n| n["pitch"] == pitch)
}
fn cursor(reply: &Value) -> (u64, u64) {
    let c = &reply["state"]["cursor"];
    (c["pitch"].as_u64().unwrap(), c["step"].as_u64().unwrap())
}
fn chord_len(reply: &Value) -> Option<usize> {
    reply["state"]["chord_preview"].as_array().map(|a| a.len())
}

#[tokio::test]
async fn full_vocabulary_demo_scenario() {
    let (url, handle) = spawn_server().await;
    let mut ws = connect(&url).await;

    // ════════════════════════════════════════════════════════════════════
    // ACT 0 — Discovery
    // ════════════════════════════════════════════════════════════════════

    // Beat 0 — the connect banner: an unsolicited frame the server sends before
    // the client says anything, naming the protocol verbs and pointing at help.
    let banner = recv(&mut ws).await;
    assert_eq!(banner["type"], "hello");
    assert!(banner["queries"]
        .as_array()
        .unwrap()
        .iter()
        .any(|q| q == "Help"));

    // Beat 0a — `query help`: structured catalog (name + params + description).
    let help = send(&mut ws, json!({ "type": "query", "what": "Help" })).await;
    assert_eq!(help["type"], "help");
    let help_names: Vec<&str> = help["actions"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(
        help_names,
        action_names(),
        "help must describe exactly action_names(), in order"
    );
    // The parametrised actions carry a param schema the agent can rely on.
    let set_cursor = help["actions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["name"] == "set_cursor")
        .unwrap();
    let pnames: Vec<&str> = set_cursor["params"]
        .as_array()
        .unwrap()
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert_eq!(pnames, vec!["pitch", "step"]);

    // Beat 0b — `query actions`: the name-only list, a subset of help.
    let acts = send(&mut ws, json!({ "type": "query", "what": "Actions" })).await;
    assert_eq!(acts["type"], "actions");
    assert_eq!(acts["actions"], json!(action_names()));

    // ════════════════════════════════════════════════════════════════════
    // ACT 1 — Grab & delete a scratch note (net zero notes)
    // ════════════════════════════════════════════════════════════════════

    // Beat 1 — set_cursor: jump to a scratch cell well below the motif.
    let r = run(&mut ws, "set_cursor", json!({ "pitch": 50, "step": 0 })).await;
    assert_eq!(cursor(&r), (50, 0));

    // Beat 2 — add_note: a throwaway note to drag around.
    let r = run(&mut ws, "add_note", json!({})).await;
    assert_eq!(note_count(&r), 1);

    // Beat 3 — toggle_grab + cursor_right: the grabbed note follows the cursor.
    run(&mut ws, "toggle_grab", json!({})).await;
    let r = run(&mut ws, "cursor_right", json!({})).await;
    assert_eq!(cursor(&r), (50, 1));
    assert_eq!(
        find_pitch(&r, 50).unwrap()["start_us"].as_u64().unwrap(),
        STEP_US,
        "grabbed note moved one step right"
    );
    run(&mut ws, "toggle_grab", json!({})).await; // drop it

    // Beat 4 — delete_note: remove the scratch note from under the cursor.
    let r = run(&mut ws, "delete_note", json!({})).await;
    assert_eq!(note_count(&r), 0, "scratch note removed; back to empty");

    // ════════════════════════════════════════════════════════════════════
    // ACT 2 — Build & shape a motif
    // ════════════════════════════════════════════════════════════════════

    // Beat 5 — set_cursor + add_note: root note C4 (pitch 60) at the start.
    run(&mut ws, "set_cursor", json!({ "pitch": 60, "step": 0 })).await;
    let r = run(&mut ws, "add_note", json!({})).await;
    assert_eq!(note_count(&r), 1);
    let n = find_pitch(&r, 60).unwrap();
    assert_eq!(n["start_us"], 0);
    assert_eq!(n["dur_us"].as_u64().unwrap(), STEP_US);
    assert_eq!(n["velocity"], 80);

    // Beat 6 — resize_note: lengthen it to two steps.
    let r = run(&mut ws, "resize_note", json!({ "delta_steps": 1 })).await;
    assert_eq!(
        find_pitch(&r, 60).unwrap()["dur_us"].as_u64().unwrap(),
        2 * STEP_US
    );

    // Beat 7 — adjust_velocity: accent it (+8 → 88).
    let r = run(&mut ws, "adjust_velocity", json!({ "delta": 8 })).await;
    assert_eq!(find_pitch(&r, 60).unwrap()["velocity"], 88);

    // ════════════════════════════════════════════════════════════════════
    // ACT 3 — Navigate every which way
    // ════════════════════════════════════════════════════════════════════

    // Beat 8 — step navigation: right then left.
    let r = run(&mut ws, "cursor_right", json!({})).await;
    assert_eq!(cursor(&r).1, 1);
    let r = run(&mut ws, "cursor_left", json!({})).await;
    assert_eq!(cursor(&r).1, 0);

    // Beat 9 — pitch navigation: semitone up/down, octave up/down.
    let r = run(&mut ws, "cursor_up", json!({})).await;
    assert_eq!(cursor(&r).0, 61);
    let r = run(&mut ws, "cursor_down", json!({})).await;
    assert_eq!(cursor(&r).0, 60);
    let r = run(&mut ws, "cursor_octave_up", json!({})).await;
    assert_eq!(cursor(&r).0, 72);
    let r = run(&mut ws, "cursor_octave_down", json!({})).await;
    assert_eq!(cursor(&r).0, 60);

    // Beat 10 — bar navigation: right jumps a whole bar, left returns.
    run(&mut ws, "set_cursor", json!({ "pitch": 60, "step": 1 })).await;
    let r = run(&mut ws, "cursor_bar_right", json!({})).await;
    let after_bar = cursor(&r).1;
    assert!(after_bar > 1, "bar right advances more than one step");
    let r = run(&mut ws, "cursor_bar_left", json!({})).await;
    assert_eq!(cursor(&r).1, 1, "bar left returns to where we started");

    // Beat 11 — jump to start / end of content.
    let r = run(&mut ws, "cursor_to_end", json!({})).await;
    assert!(cursor(&r).1 >= 1, "cursor_to_end lands on/after content");
    let r = run(&mut ws, "cursor_to_start", json!({})).await;
    assert_eq!(cursor(&r).1, 0);

    // Beat 12 — subdivision finer then coarser (round-trips to Sixteenth).
    let r = run(&mut ws, "subdivision_finer", json!({})).await;
    assert_eq!(r["state"]["subdivision"], "ThirtySecond");
    let r = run(&mut ws, "subdivision_coarser", json!({})).await;
    assert_eq!(r["state"]["subdivision"], "Sixteenth");

    // ════════════════════════════════════════════════════════════════════
    // ACT 4 — A second note, then chords
    // ════════════════════════════════════════════════════════════════════

    // Beat 13 — set_cursor + add_note: E4 (pitch 64) at step 4.
    run(&mut ws, "set_cursor", json!({ "pitch": 64, "step": 4 })).await;
    let r = run(&mut ws, "add_note", json!({})).await;
    assert_eq!(note_count(&r), 2);
    let melody_notes = note_count(&r);

    // Beat 14 — enter_chord_mode: a triad preview appears (3 ghost notes).
    run(&mut ws, "set_cursor", json!({ "pitch": 67, "step": 8 })).await;
    let r = run(&mut ws, "enter_chord_mode", json!({})).await;
    assert_eq!(chord_len(&r), Some(3), "triad preview = 3 pitches");
    assert_eq!(note_count(&r), melody_notes + 3);

    // Beat 15 — cycle_chord_degree / set_chord_degree: still a triad.
    let r = run(&mut ws, "cycle_chord_degree", json!({ "delta": 1 })).await;
    assert_eq!(chord_len(&r), Some(3));
    let r = run(&mut ws, "set_chord_degree", json!({ "degree": 1 })).await;
    assert_eq!(chord_len(&r), Some(3));

    // Beat 16 — toggle_chord_kind: triad → seventh (4 pitches).
    let r = run(&mut ws, "toggle_chord_kind", json!({})).await;
    assert_eq!(chord_len(&r), Some(4), "seventh preview = 4 pitches");
    assert_eq!(note_count(&r), melody_notes + 4);

    // Beat 17 — commit_chord: preview becomes permanent, selector closes.
    let r = run(&mut ws, "commit_chord", json!({})).await;
    assert!(chord_len(&r).is_none(), "no preview after commit");
    assert_eq!(note_count(&r), melody_notes + 4);
    let after_commit = note_count(&r);

    // Beat 18 — enter_chord_mode + cancel_chord: preview rolls back cleanly.
    run(&mut ws, "set_cursor", json!({ "pitch": 72, "step": 12 })).await;
    let r = run(&mut ws, "enter_chord_mode", json!({})).await;
    assert!(note_count(&r) > after_commit);
    let r = run(&mut ws, "cancel_chord", json!({})).await;
    assert!(chord_len(&r).is_none());
    assert_eq!(
        note_count(&r),
        after_commit,
        "cancel removes the preview entirely"
    );

    // ════════════════════════════════════════════════════════════════════
    // ACT 5 — Input mode (record arm / flavour)
    // ════════════════════════════════════════════════════════════════════

    // Beat 19 — arm recording, flip flavour, disarm.
    let r = run(&mut ws, "toggle_record_arm", json!({})).await;
    assert_eq!(r["state"]["input_mode"], "StepRecord");
    let r = run(&mut ws, "toggle_record_flavour", json!({})).await;
    assert_eq!(r["state"]["input_mode"], "LiveRecord");
    let r = run(&mut ws, "toggle_record_arm", json!({})).await;
    assert_eq!(r["state"]["input_mode"], "DirectEdit");

    // ════════════════════════════════════════════════════════════════════
    // ACT 6 — Transport (pure: time is injected, never wall-clock)
    // ════════════════════════════════════════════════════════════════════

    // Beat 20 — toggle_play_cursor on, then off.
    let r = run(&mut ws, "toggle_play_cursor", json!({})).await;
    assert_eq!(r["state"]["playing"], true);
    let r = run(&mut ws, "toggle_play_cursor", json!({})).await;
    assert_eq!(r["state"]["playing"], false);

    // Beat 21 — play_from_start: playhead at 0.
    let r = run(&mut ws, "play_from_start", json!({})).await;
    assert_eq!(r["state"]["playing"], true);
    assert_eq!(r["state"]["playhead_us"], 0);

    // Beat 22 — play{from_us}: playhead jumps to the requested offset.
    let r = run(&mut ws, "play", json!({ "from_us": 500_000 })).await;
    assert_eq!(r["state"]["playhead_us"], 500_000);

    // Beat 23 — stop.
    let r = run(&mut ws, "stop", json!({})).await;
    assert_eq!(r["state"]["playing"], false);

    // Beat 24 — set_playhead: moves the record playhead (not asserted in the
    // stopped snapshot, where playhead_us tracks the cursor); just must succeed.
    run(&mut ws, "set_playhead", json!({ "us": 1_000_000 })).await;

    // ════════════════════════════════════════════════════════════════════
    // ACT 7 — Loop, metronome, count-in
    // ════════════════════════════════════════════════════════════════════

    // Beat 25 — toggle_metronome on.
    let r = run(&mut ws, "toggle_metronome", json!({})).await;
    assert_eq!(r["state"]["metronome"], true);

    // Beat 26 — set_loop_bounds + toggle_loop.
    let r = run(
        &mut ws,
        "set_loop_bounds",
        json!({ "start_us": 0, "end_us": 1_000_000 }),
    )
    .await;
    assert_eq!(r["state"]["loop_start_us"], 0);
    assert_eq!(r["state"]["loop_end_us"], 1_000_000);
    let r = run(&mut ws, "toggle_loop", json!({})).await;
    assert_eq!(r["state"]["looping"], true);

    // Beat 27 — start_count_in_record: enters live-record and starts playing.
    let r = run(&mut ws, "start_count_in_record", json!({})).await;
    assert_eq!(r["state"]["input_mode"], "LiveRecord");
    assert_eq!(r["state"]["playing"], true);
    // Reset transport/mode so the selection act below is deterministic.
    run(&mut ws, "stop", json!({})).await;
    run(&mut ws, "toggle_loop", json!({})).await;
    run(&mut ws, "toggle_metronome", json!({})).await;

    // ════════════════════════════════════════════════════════════════════
    // ACT 8 — Selection & clipboard
    // ════════════════════════════════════════════════════════════════════

    // Beat 28 — start_selection then extend it by moving the cursor.
    run(&mut ws, "set_cursor", json!({ "pitch": 60, "step": 0 })).await;
    let r = run(&mut ws, "start_selection", json!({})).await;
    assert!(!r["state"]["selection"].is_null(), "selection active");
    let r = run(&mut ws, "set_cursor", json!({ "pitch": 84, "step": 16 })).await;
    let sel = &r["state"]["selection"];
    assert_eq!(sel["pitch_lo"], 60);
    assert_eq!(sel["pitch_hi"], 84);
    assert_eq!(sel["us_lo"], 0);

    // Beat 29 — yank_selection: notes copied; selection clears.
    let before_yank = note_count(&r);
    let r = run(&mut ws, "yank_selection", json!({})).await;
    let clip = r["state"]["clipboard_len"].as_u64().unwrap();
    assert!(clip >= 2, "clipboard captured the motif + chord");
    assert!(
        r["state"]["selection"].is_null(),
        "selection cleared by yank"
    );
    assert_eq!(
        note_count(&r),
        before_yank,
        "yank doesn't change the timeline"
    );

    // Beat 30 — paste_clipboard at a fresh cell: notes grow by clipboard_len.
    run(&mut ws, "set_cursor", json!({ "pitch": 48, "step": 32 })).await;
    let before_paste = before_yank;
    let r = run(&mut ws, "paste_clipboard", json!({})).await;
    assert_eq!(note_count(&r), before_paste + clip as usize);

    // Beat 31 — start_selection + clear_selection: cancel a selection.
    run(&mut ws, "start_selection", json!({})).await;
    run(&mut ws, "set_cursor", json!({ "pitch": 72, "step": 40 })).await;
    let r = run(&mut ws, "clear_selection", json!({})).await;
    assert!(r["state"]["selection"].is_null(), "selection cleared");

    // Beat 32 — select-all-ish then delete_selection: notes drop.
    run(&mut ws, "set_cursor", json!({ "pitch": 0, "step": 0 })).await;
    run(&mut ws, "start_selection", json!({})).await;
    let r = run(&mut ws, "set_cursor", json!({ "pitch": 108, "step": 64 })).await;
    let before_delete = note_count(&r);
    let r = run(&mut ws, "delete_selection", json!({})).await;
    assert!(note_count(&r) < before_delete, "selection deleted");
    let after_delete = note_count(&r);

    // ════════════════════════════════════════════════════════════════════
    // ACT 9 — History
    // ════════════════════════════════════════════════════════════════════

    // Beat 33 — undo brings the deleted notes back; redo removes them again.
    let r = run(&mut ws, "undo", json!({})).await;
    assert!(note_count(&r) > after_delete, "undo restores deleted notes");
    let restored = note_count(&r);
    let r = run(&mut ws, "redo", json!({})).await;
    assert!(note_count(&r) < restored, "redo re-applies the delete");

    // ════════════════════════════════════════════════════════════════════
    // ACT 10 — Protocol features (not actions): state / render / subscribe
    // ════════════════════════════════════════════════════════════════════

    // Beat 34 — query state mirrors the last run_action snapshot.
    let q = send(&mut ws, json!({ "type": "query", "what": "State" })).await;
    assert_eq!(q["type"], "ok");
    assert!(q["state"]["notes"].is_array());

    // Beat 35 — query render returns the text screenshot channel.
    let q = send(&mut ws, json!({ "type": "query", "what": "Render" })).await;
    assert_eq!(q["type"], "render");
    assert!(q["text"].is_string());

    // Beat 36 — subscribe / unsubscribe to the events topic.
    let s = send(&mut ws, json!({ "type": "subscribe", "topic": "Events" })).await;
    assert_eq!(s["type"], "ok");
    let u = send(&mut ws, json!({ "type": "unsubscribe", "topic": "Events" })).await;
    assert_eq!(u["type"], "ok");

    handle.abort();
}
