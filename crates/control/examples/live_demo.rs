//! Live run of `docs/DEMO-SCENARIO.md` against the **production** control seam.
//!
//! Unlike the integration test (which uses `ControlServer`), this drives a
//! `CommandServer` that routes every request over an mpsc channel to a
//! stand-in app loop owning a single `Composer` — exactly how the live TUI is
//! wired in `crates/tui/src/main.rs`. It connects over a real loopback socket,
//! reads the connect banner, runs all the beats, and prints a transcript.
//!
//!   cargo run -p rockcraft-control --example live_demo

use std::sync::atomic::{AtomicI32, Ordering};

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use rockcraft_control::{handle, CommandServer, RemoteCommand};
use rockcraft_core::Composer;

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

static PASS: AtomicI32 = AtomicI32::new(0);
static FAIL: AtomicI32 = AtomicI32::new(0);

const STEP_US: u64 = 125_000;

async fn recv(ws: &mut Ws) -> Value {
    loop {
        match ws.next().await.unwrap().unwrap() {
            Message::Text(t) => return serde_json::from_str(&t).unwrap(),
            _ => continue,
        }
    }
}

async fn send(ws: &mut Ws, req: Value) -> Value {
    ws.send(Message::Text(req.to_string())).await.unwrap();
    loop {
        let v = recv(ws).await;
        if v["type"] == "hello" || v["type"] == "event" {
            continue;
        }
        return v;
    }
}

async fn run(ws: &mut Ws, action: &str, params: Value) -> Value {
    send(
        ws,
        json!({ "type": "run_action", "action": action, "params": params }),
    )
    .await
}

/// Print one beat line with a pass/fail mark.
fn check(beat: &str, ok: bool, detail: &str) {
    if ok {
        PASS.fetch_add(1, Ordering::Relaxed);
        println!("  \u{2713} {beat:<46} {detail}");
    } else {
        FAIL.fetch_add(1, Ordering::Relaxed);
        println!("  \u{2717} {beat:<46} {detail}   <-- FAIL");
    }
}

fn ncount(r: &Value) -> usize {
    r["state"]["notes"].as_array().map(|a| a.len()).unwrap_or(0)
}
fn cur(r: &Value) -> (u64, u64) {
    let c = &r["state"]["cursor"];
    (c["pitch"].as_u64().unwrap(), c["step"].as_u64().unwrap())
}
fn vel(r: &Value, pitch: u64) -> u64 {
    r["state"]["notes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|n| n["pitch"] == pitch)
        .map(|n| n["velocity"].as_u64().unwrap())
        .unwrap_or(0)
}
fn chord_len(r: &Value) -> Option<usize> {
    r["state"]["chord_preview"].as_array().map(|a| a.len())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // ── Production seam: CommandServer → channel → app loop owning a Composer ──
    let (tx, mut rx) = mpsc::channel::<RemoteCommand>(32);
    let server = CommandServer::bind("127.0.0.1:0", tx).await?;
    let url = format!("ws://{}", server.local_addr());
    tokio::spawn(async move {
        let _ = server.serve().await;
    });
    tokio::spawn(async move {
        let mut composer = Composer::new();
        while let Some(cmd) = rx.recv().await {
            let response = handle(&mut composer, cmd.req);
            let _ = cmd.reply.send(response);
        }
    });

    println!("Connecting to {url} (production CommandServer seam)\n");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await?;

    // ── Act 0: banner + discovery ──
    println!("Act 0 — Discovery");
    let banner = recv(&mut ws).await;
    check(
        "Beat 0  connect banner",
        banner["type"] == "hello",
        &format!(
            "type=hello queries={}",
            banner["queries"].as_array().map(|a| a.len()).unwrap_or(0)
        ),
    );
    let help = send(&mut ws, json!({"type":"query","what":"Help"})).await;
    let nactions = help["actions"].as_array().map(|a| a.len()).unwrap_or(0);
    check(
        "Beat 0a query Help",
        help["type"] == "help" && nactions == 42,
        &format!("{nactions} actions described"),
    );
    let acts = send(&mut ws, json!({"type":"query","what":"Actions"})).await;
    check(
        "Beat 0b query Actions",
        acts["actions"].as_array().map(|a| a.len()).unwrap_or(0) == nactions,
        "names match help",
    );

    // ── Act 1: grab & delete ──
    println!("Act 1 — Grab & delete a scratch note");
    let r = run(&mut ws, "set_cursor", json!({"pitch":50,"step":0})).await;
    check(
        "Beat 1  set_cursor(50,0)",
        cur(&r) == (50, 0),
        "cursor=(50,0)",
    );
    let r = run(&mut ws, "add_note", json!({})).await;
    check("Beat 2  add_note", ncount(&r) == 1, "1 note");
    run(&mut ws, "toggle_grab", json!({})).await;
    let r = run(&mut ws, "cursor_right", json!({})).await;
    let moved = r["state"]["notes"][0]["start_us"].as_u64().unwrap() == STEP_US;
    check(
        "Beat 3  toggle_grab + cursor_right",
        moved,
        "note dragged to step 1",
    );
    run(&mut ws, "toggle_grab", json!({})).await;
    let r = run(&mut ws, "delete_note", json!({})).await;
    check("Beat 4  delete_note", ncount(&r) == 0, "back to empty");

    // ── Act 2: build & shape a motif ──
    println!("Act 2 — Build & shape a motif");
    run(&mut ws, "set_cursor", json!({"pitch":60,"step":0})).await;
    let r = run(&mut ws, "add_note", json!({})).await;
    check(
        "Beat 5  add_note C4",
        ncount(&r) == 1 && vel(&r, 60) == 80,
        "vel=80",
    );
    let r = run(&mut ws, "resize_note", json!({"delta_steps":1})).await;
    let dur = r["state"]["notes"][0]["dur_us"].as_u64().unwrap();
    check("Beat 6  resize_note +1", dur == 2 * STEP_US, "dur=2 steps");
    let r = run(&mut ws, "adjust_velocity", json!({"delta":8})).await;
    check("Beat 7  adjust_velocity +8", vel(&r, 60) == 88, "vel=88");

    // ── Act 3: navigation ──
    println!("Act 3 — Navigate");
    let r = run(&mut ws, "cursor_right", json!({})).await;
    let a = cur(&r).1 == 1;
    let r = run(&mut ws, "cursor_left", json!({})).await;
    check(
        "Beat 8  cursor_right/left",
        a && cur(&r).1 == 0,
        "step 1 then 0",
    );
    let r = run(&mut ws, "cursor_up", json!({})).await;
    let a = cur(&r).0 == 61;
    let r = run(&mut ws, "cursor_octave_up", json!({})).await;
    let b = cur(&r).0 == 73;
    let r = run(&mut ws, "cursor_octave_down", json!({})).await;
    let r2 = run(&mut ws, "cursor_down", json!({})).await;
    check(
        "Beat 9  up/octave up/down/down",
        a && b && cur(&r).0 == 61 && cur(&r2).0 == 60,
        "pitch 61→73→61→60",
    );
    run(&mut ws, "set_cursor", json!({"pitch":60,"step":1})).await;
    let r = run(&mut ws, "cursor_bar_right", json!({})).await;
    let far = cur(&r).1;
    let r = run(&mut ws, "cursor_bar_left", json!({})).await;
    check(
        "Beat 10 cursor_bar_right/left",
        far == 17 && cur(&r).1 == 1,
        &format!("step 1→{far}→1"),
    );
    let r = run(&mut ws, "cursor_to_end", json!({})).await;
    let a = cur(&r).1 >= 1;
    let r = run(&mut ws, "cursor_to_start", json!({})).await;
    check(
        "Beat 11 cursor_to_end/start",
        a && cur(&r).1 == 0,
        "to end then 0",
    );
    let r = run(&mut ws, "subdivision_finer", json!({})).await;
    let a = r["state"]["subdivision"] == "ThirtySecond";
    let r = run(&mut ws, "subdivision_coarser", json!({})).await;
    check(
        "Beat 12 subdivision finer/coarser",
        a && r["state"]["subdivision"] == "Sixteenth",
        "32nd then 16th",
    );

    // ── Act 4: second note + chords ──
    println!("Act 4 — Second note, then chords");
    run(&mut ws, "set_cursor", json!({"pitch":64,"step":4})).await;
    let r = run(&mut ws, "add_note", json!({})).await;
    let melody = ncount(&r);
    check("Beat 13 add_note E4", melody == 2, "2 notes");
    run(&mut ws, "set_cursor", json!({"pitch":67,"step":8})).await;
    let r = run(&mut ws, "enter_chord_mode", json!({})).await;
    check(
        "Beat 14 enter_chord_mode",
        chord_len(&r) == Some(3) && ncount(&r) == melody + 3,
        "triad preview = 3",
    );
    run(&mut ws, "cycle_chord_degree", json!({"delta":1})).await;
    let r = run(&mut ws, "set_chord_degree", json!({"degree":1})).await;
    check(
        "Beat 15 cycle/set_chord_degree",
        chord_len(&r) == Some(3),
        "still triad",
    );
    let r = run(&mut ws, "toggle_chord_kind", json!({})).await;
    check(
        "Beat 16 toggle_chord_kind",
        chord_len(&r) == Some(4),
        "seventh = 4",
    );
    let r = run(&mut ws, "commit_chord", json!({})).await;
    let after_commit = ncount(&r);
    check(
        "Beat 17 commit_chord",
        chord_len(&r).is_none() && after_commit == melody + 4,
        "committed, no preview",
    );
    run(&mut ws, "set_cursor", json!({"pitch":72,"step":12})).await;
    run(&mut ws, "enter_chord_mode", json!({})).await;
    let r = run(&mut ws, "cancel_chord", json!({})).await;
    check(
        "Beat 18 enter + cancel_chord",
        chord_len(&r).is_none() && ncount(&r) == after_commit,
        "rolled back",
    );

    // ── Act 5: input mode ──
    println!("Act 5 — Input mode");
    let r = run(&mut ws, "toggle_record_arm", json!({})).await;
    let a = r["state"]["input_mode"] == "StepRecord";
    let r = run(&mut ws, "toggle_record_flavour", json!({})).await;
    let b = r["state"]["input_mode"] == "LiveRecord";
    let r = run(&mut ws, "toggle_record_arm", json!({})).await;
    check(
        "Beat 19 record arm/flavour/disarm",
        a && b && r["state"]["input_mode"] == "DirectEdit",
        "Step→Live→DirectEdit",
    );

    // ── Act 6: transport ──
    println!("Act 6 — Transport");
    let r = run(&mut ws, "toggle_play_cursor", json!({})).await;
    let a = r["state"]["playing"] == true;
    let r = run(&mut ws, "toggle_play_cursor", json!({})).await;
    check(
        "Beat 20 toggle_play_cursor x2",
        a && r["state"]["playing"] == false,
        "on then off",
    );
    let r = run(&mut ws, "play_from_start", json!({})).await;
    check(
        "Beat 21 play_from_start",
        r["state"]["playing"] == true && r["state"]["playhead_us"] == 0,
        "playhead=0",
    );
    let r = run(&mut ws, "play", json!({"from_us":500000})).await;
    check(
        "Beat 22 play{from_us}",
        r["state"]["playhead_us"] == 500000,
        "playhead=500000",
    );
    let r = run(&mut ws, "stop", json!({})).await;
    check("Beat 23 stop", r["state"]["playing"] == false, "stopped");
    let r = run(&mut ws, "set_playhead", json!({"us":1000000})).await;
    check("Beat 24 set_playhead", r["type"] == "ok", "ok");

    // ── Act 7: loop / metronome / count-in ──
    println!("Act 7 — Loop, metronome, count-in");
    let r = run(&mut ws, "toggle_metronome", json!({})).await;
    check(
        "Beat 25 toggle_metronome",
        r["state"]["metronome"] == true,
        "on",
    );
    let r = run(
        &mut ws,
        "set_loop_bounds",
        json!({"start_us":0,"end_us":1000000}),
    )
    .await;
    let a = r["state"]["loop_end_us"] == 1000000;
    let r = run(&mut ws, "toggle_loop", json!({})).await;
    check(
        "Beat 26 set_loop_bounds + toggle_loop",
        a && r["state"]["looping"] == true,
        "loop set & on",
    );
    let r = run(&mut ws, "start_count_in_record", json!({})).await;
    check(
        "Beat 27 start_count_in_record",
        r["state"]["input_mode"] == "LiveRecord" && r["state"]["playing"] == true,
        "live-record + playing",
    );
    run(&mut ws, "stop", json!({})).await;
    run(&mut ws, "toggle_loop", json!({})).await;
    run(&mut ws, "toggle_metronome", json!({})).await;

    // ── Act 8: selection / clipboard ──
    println!("Act 8 — Selection & clipboard");
    run(&mut ws, "set_cursor", json!({"pitch":60,"step":0})).await;
    run(&mut ws, "start_selection", json!({})).await;
    let r = run(&mut ws, "set_cursor", json!({"pitch":84,"step":16})).await;
    let sel = &r["state"]["selection"];
    check(
        "Beat 28 start_selection + extend",
        !sel.is_null() && sel["pitch_lo"] == 60 && sel["pitch_hi"] == 84,
        "rect 60..84",
    );
    let before = ncount(&r);
    let r = run(&mut ws, "yank_selection", json!({})).await;
    let clip = r["state"]["clipboard_len"].as_u64().unwrap();
    check(
        "Beat 29 yank_selection",
        clip >= 2 && r["state"]["selection"].is_null() && ncount(&r) == before,
        &format!("clipboard={clip}"),
    );
    run(&mut ws, "set_cursor", json!({"pitch":48,"step":32})).await;
    let r = run(&mut ws, "paste_clipboard", json!({})).await;
    check(
        "Beat 30 paste_clipboard",
        ncount(&r) == before + clip as usize,
        &format!("notes {} -> {}", before, ncount(&r)),
    );
    run(&mut ws, "start_selection", json!({})).await;
    run(&mut ws, "set_cursor", json!({"pitch":72,"step":40})).await;
    let r = run(&mut ws, "clear_selection", json!({})).await;
    check(
        "Beat 31 clear_selection",
        r["state"]["selection"].is_null(),
        "cleared",
    );
    run(&mut ws, "set_cursor", json!({"pitch":0,"step":0})).await;
    run(&mut ws, "start_selection", json!({})).await;
    let r = run(&mut ws, "set_cursor", json!({"pitch":108,"step":64})).await;
    let before_del = ncount(&r);
    let r = run(&mut ws, "delete_selection", json!({})).await;
    let after_del = ncount(&r);
    check(
        "Beat 32 delete_selection",
        after_del < before_del,
        &format!("notes {before_del} -> {after_del}"),
    );

    // ── Act 9: history ──
    println!("Act 9 — History");
    let r = run(&mut ws, "undo", json!({})).await;
    let restored = ncount(&r);
    let a = restored > after_del;
    let r = run(&mut ws, "redo", json!({})).await;
    check(
        "Beat 33 undo / redo",
        a && ncount(&r) < restored,
        &format!("undo→{restored}, redo→{}", ncount(&r)),
    );

    // ── Act 10: protocol features ──
    println!("Act 10 — Protocol features");
    let q = send(&mut ws, json!({"type":"query","what":"State"})).await;
    check("Beat 34 query State", q["type"] == "ok", "snapshot");
    let q = send(&mut ws, json!({"type":"query","what":"Render"})).await;
    check(
        "Beat 35 query Render",
        q["type"] == "render",
        "render channel",
    );
    let s = send(&mut ws, json!({"type":"subscribe","topic":"Events"})).await;
    let u = send(&mut ws, json!({"type":"unsubscribe","topic":"Events"})).await;
    check(
        "Beat 36 subscribe/unsubscribe",
        s["type"] == "ok" && u["type"] == "ok",
        "ok/ok",
    );

    let (p, f) = (PASS.load(Ordering::Relaxed), FAIL.load(Ordering::Relaxed));
    println!("\n=== {p} beats passed, {f} failed ===");
    if f > 0 {
        std::process::exit(1);
    }
    Ok(())
}
