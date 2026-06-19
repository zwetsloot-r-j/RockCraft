//! AI-agent scenario driver: edit a new song with a movie backing, save it,
//! play it back, then shut the app down — all over the control socket.
//!
//! This is the executable twin of `docs/BACKING-MOVIE-SCENARIO.md`. It launches
//! the **real Tauri desktop app** with the control socket enabled, connects as
//! an agent, drives the full authoring→playback loop, asserts the song was
//! persisted correctly, and finally tells the app to quit (`app_quit`) so the
//! whole run — start to shutdown — is autonomous.
//!
//! Usage:
//!   cargo build --bin rockcraft-tauri
//!   cargo run -p rockcraft-control --example backing_movie_session
//!
//! Env overrides:
//!   ROCKCRAFT_TAURI_BIN   path to the built rockcraft-tauri binary
//!                         (default: target/debug/rockcraft-tauri)
//!   ROCKCRAFT_CONTROL_ADDR  pinned control address (default 127.0.0.1:9001)
//!
//! On a headless host (e.g. WSL/CI) the GUI needs a display; if `xvfb-run` is on
//! PATH and `DISPLAY` is unset, the app is launched under it automatically.

use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio_tungstenite::tungstenite::protocol::Message;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

type Ws = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// A scenario step failed an assertion or a request errored.
fn bail(msg: impl Into<String>) -> Box<dyn std::error::Error> {
    msg.into().into()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let addr =
        std::env::var("ROCKCRAFT_CONTROL_ADDR").unwrap_or_else(|_| "127.0.0.1:9001".to_string());
    let url = format!("ws://{addr}");

    // ── Stage a temp working dir with a "movie" file ────────────────────────
    // QuickSave writes to `recordings/…` relative to the app's cwd, so a temp
    // cwd keeps every artifact contained and easy to clean up. No media is ever
    // committed to git — the clip is generated here at runtime.
    let work = make_temp_dir()?;
    let movie = work.join("movie.mp4");
    write_movie(&movie)?;
    println!("• staged movie backing at {}", movie.display());

    // ── Start the game ──────────────────────────────────────────────────────
    let mut child = spawn_app(&work, &addr)?;
    println!("• launched rockcraft-tauri (pid {})", child.id());

    // Run the scenario; always tear down the child + temp dir afterwards.
    let result = run_scenario(&url, &movie, &work).await;

    // Best-effort cleanup: app_quit should have exited it already.
    std::thread::sleep(Duration::from_millis(300));
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&work);

    match result {
        Ok(()) => {
            println!("\nSCENARIO OK");
            Ok(())
        }
        Err(e) => {
            eprintln!("\nSCENARIO FAILED: {e}");
            Err(e)
        }
    }
}

async fn run_scenario(
    url: &str,
    movie: &Path,
    work: &Path,
) -> Result<(), Box<dyn std::error::Error>> {
    // ── Connect ─────────────────────────────────────────────────────────────
    let mut ws = connect_with_retry(url, Duration::from_secs(30)).await?;
    println!("• connected to {url}");

    // Beat 0 — discover the vocabulary live; confirm the movie + quit + play
    // commands the scenario needs are present.
    let help = send(&mut ws, json!({ "type": "query", "id": 0, "what": "Help" })).await?;
    let host_cmds: Vec<String> = help["host_commands"]
        .as_array()
        .ok_or_else(|| bail("help has no host_commands"))?
        .iter()
        .filter_map(|c| c["name"].as_str().map(str::to_string))
        .collect();
    for needed in [
        "attach_video",
        "query_video",
        "save_bundle",
        "play_load",
        "app_quit",
    ] {
        if !host_cmds.contains(&needed.to_string()) {
            return Err(bail(format!("help is missing host command `{needed}`")));
        }
    }
    println!("• query Help lists attach_video / play_load / app_quit ✓");

    // Beat 1 — attach the movie as backing (the gap this scenario exists for).
    let v = host(
        &mut ws,
        1,
        "attach_video",
        json!({ "path": movie.to_string_lossy(), "offset_us": -100_000 }),
    )
    .await?;
    if v["path"].as_str() != movie.to_string_lossy().as_ref().into() {
        return Err(bail(format!("attach_video echoed wrong path: {v}")));
    }
    println!("• attach_video → {}", v);

    // Beat 2 — author a short melody: four ascending notes, one per bar-quarter.
    let melody = [(60u8, 0u64), (62, 4), (64, 8), (65, 12)];
    for (i, (pitch, step)) in melody.iter().enumerate() {
        action(
            &mut ws,
            (100 + i) as u64,
            "set_cursor",
            json!({ "pitch": pitch, "step": step }),
        )
        .await?;
        action(&mut ws, (200 + i) as u64, "add_note", json!({})).await?;
    }
    let state = send(
        &mut ws,
        json!({ "type": "query", "id": 3, "what": "State" }),
    )
    .await?;
    let note_count = state["state"]["notes"]
        .as_array()
        .map(|a| a.len())
        .unwrap_or(0);
    if note_count != melody.len() {
        return Err(bail(format!(
            "expected {} notes, got {note_count}",
            melody.len()
        )));
    }
    println!("• authored {note_count} notes ✓");

    // Beat 3 — there are unsaved edits.
    let dirty = host(&mut ws, 4, "query_dirty", json!({})).await?;
    if dirty != json!(true) {
        return Err(bail(format!(
            "expected dirty=true before save, got {dirty}"
        )));
    }

    // Beat 4 — save the song as a bundle (persists song.mid + meta.json + movie).
    let saved = host(
        &mut ws,
        5,
        "save_bundle",
        json!({ "dest": { "kind": "quick_save" } }),
    )
    .await?;
    let dir_rel = saved["dir"]
        .as_str()
        .ok_or_else(|| bail(format!("save_bundle returned no dir: {saved}")))?
        .to_string();
    // The app's cwd is `work`; resolve the (relative) quick-save dir against it.
    let bundle_dir = work.join(&dir_rel);
    println!("• save_bundle → {}", bundle_dir.display());

    // Beat 5 — the save cleared the dirty flag, and the bundle is on disk with
    // the movie reference persisted into meta.json.
    let dirty = host(&mut ws, 6, "query_dirty", json!({})).await?;
    if dirty != json!(false) {
        return Err(bail(format!(
            "expected dirty=false after save, got {dirty}"
        )));
    }
    let mid = bundle_dir.join("song.mid");
    let meta_path = bundle_dir.join("meta.json");
    if !mid.exists() {
        return Err(bail(format!("song.mid not written at {}", mid.display())));
    }
    let meta: Value = serde_json::from_str(&std::fs::read_to_string(&meta_path)?)?;
    if meta["video"].is_null() {
        return Err(bail(format!("meta.json has no video block: {meta}")));
    }
    println!("• bundle persisted: song.mid + meta.json (with video) ✓");

    // Beat 6 — load the saved bundle in playback mode and confirm the full loop:
    // the notes and the movie both come back through play_load.
    let info = host(
        &mut ws,
        7,
        "play_load",
        json!({ "dir": bundle_dir.to_string_lossy() }),
    )
    .await?;
    let played = info["notes"].as_array().map(|a| a.len()).unwrap_or(0);
    if played != melody.len() {
        return Err(bail(format!(
            "play_load surfaced {played} notes, expected {}",
            melody.len()
        )));
    }
    if info["video"].is_null() {
        return Err(bail(format!("play_load lost the movie backing: {info}")));
    }
    println!("• play_load → {played} notes + movie backing ✓");

    // Beat 7 — finish the play session; the scoring loop returns a summary.
    let summary = host(&mut ws, 8, "play_finish", json!({})).await?;
    println!("• play_finish → {}", summary);

    // Beat 8 — close the game down. app_quit exits the process, so the socket
    // closes; a clean reply or a connection close both count as success.
    println!("• app_quit → shutting the game down");
    let _ = ws
        .send(Message::Text(
            json!({ "type": "run_host_command", "id": 9, "command": "app_quit", "params": {} })
                .to_string(),
        ))
        .await;
    // Drain whatever comes back (a host_result or a close frame) without failing.
    let _ = tokio::time::timeout(Duration::from_secs(3), recv_any(&mut ws)).await;

    Ok(())
}

// ── WebSocket helpers ──────────────────────────────────────────────────────

/// Send one request and return the correlated response, skipping unsolicited
/// `hello`/`event` frames.
async fn send(ws: &mut Ws, req: Value) -> Result<Value, Box<dyn std::error::Error>> {
    ws.send(Message::Text(req.to_string())).await?;
    loop {
        match recv_any(ws).await? {
            Some(v) if v["type"] == "hello" || v["type"] == "event" => continue,
            Some(v) => return Ok(v),
            None => return Err(bail("connection closed awaiting response")),
        }
    }
}

/// Run a composer action; return its post-edit response, erroring on `err`.
async fn action(
    ws: &mut Ws,
    id: u64,
    name: &str,
    params: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let resp = send(
        ws,
        json!({ "type": "run_action", "id": id, "action": name, "params": params }),
    )
    .await?;
    if resp["type"] == "err" {
        return Err(bail(format!("action {name} failed: {}", resp["error"])));
    }
    Ok(resp)
}

/// Run a host command; return the inner `value`, erroring on `err`.
async fn host(
    ws: &mut Ws,
    id: u64,
    name: &str,
    params: Value,
) -> Result<Value, Box<dyn std::error::Error>> {
    let resp = send(
        ws,
        json!({ "type": "run_host_command", "id": id, "command": name, "params": params }),
    )
    .await?;
    if resp["type"] == "err" {
        return Err(bail(format!(
            "host command {name} failed: {}",
            resp["error"]
        )));
    }
    Ok(resp["value"].clone())
}

/// Read the next text frame as JSON; `Ok(None)` on a clean close.
async fn recv_any(ws: &mut Ws) -> Result<Option<Value>, Box<dyn std::error::Error>> {
    while let Some(msg) = ws.next().await {
        match msg? {
            Message::Text(t) => return Ok(Some(serde_json::from_str(&t)?)),
            Message::Close(_) => return Ok(None),
            _ => continue,
        }
    }
    Ok(None)
}

async fn connect_with_retry(url: &str, budget: Duration) -> Result<Ws, Box<dyn std::error::Error>> {
    let deadline = tokio::time::Instant::now() + budget;
    loop {
        match connect_async(url).await {
            Ok((ws, _)) => return Ok(ws),
            Err(e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(bail(format!("could not connect to {url}: {e}")));
                }
                tokio::time::sleep(Duration::from_millis(250)).await;
            }
        }
    }
}

// ── Process / fixture helpers ──────────────────────────────────────────────

fn spawn_app(work: &Path, addr: &str) -> Result<Child, Box<dyn std::error::Error>> {
    let bin = std::env::var("ROCKCRAFT_TAURI_BIN")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("target/debug/rockcraft-tauri"));
    if !bin.exists() {
        return Err(bail(format!(
            "{} not found — run `cargo build --bin rockcraft-tauri` first (or set ROCKCRAFT_TAURI_BIN)",
            bin.display()
        )));
    }

    // On a headless host, run under xvfb-run when available so the webview can
    // start without a real display.
    let headless = std::env::var_os("DISPLAY").is_none();
    let mut cmd = if headless && on_path("xvfb-run") {
        println!("• no DISPLAY; launching under xvfb-run");
        let mut c = Command::new("xvfb-run");
        c.arg("-a").arg(&bin);
        c
    } else {
        Command::new(&bin)
    };
    cmd.arg("--control")
        .current_dir(work)
        .env("ROCKCRAFT_CONTROL_ADDR", addr);
    Ok(cmd.spawn()?)
}

/// Write the backing "movie". Uses ffmpeg to synthesise a tiny real clip when
/// available (nicer for a manual watch); otherwise a small placeholder file —
/// the loop only *references* the file, it is never decoded by the test.
fn write_movie(path: &Path) -> std::io::Result<()> {
    if on_path("ffmpeg") {
        let ok = Command::new("ffmpeg")
            .args([
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=2:size=320x240:rate=15",
                "-pix_fmt",
                "yuv420p",
            ])
            .arg(path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
    }
    // Placeholder: a few KB so a copy is meaningful. Not a decodable video.
    std::fs::write(path, vec![0u8; 4096])
}

fn make_temp_dir() -> std::io::Result<PathBuf> {
    let base = std::env::temp_dir().join(format!("rockcraft-backing-movie-{}", std::process::id()));
    std::fs::create_dir_all(&base)?;
    Ok(base)
}

/// Whether `name` resolves on `PATH`.
fn on_path(name: &str) -> bool {
    std::env::var_os("PATH")
        .map(|paths| {
            std::env::split_paths(&paths).any(|dir| {
                let p = dir.join(name);
                p.is_file()
            })
        })
        .unwrap_or(false)
}
