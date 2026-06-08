//! Minimal example: connect to a running RockCraft control server and drive edits.
//!
//! Usage:
//!   cargo run --example agent_session -- --port PORT
//!
//! Where PORT is the control server port (logged when the TUI starts with --control).
//!
//! This example:
//! 1. Connects to ws://127.0.0.1:PORT
//! 2. Adds a note at the current cursor position
//! 3. Moves the cursor right
//! 4. Adds another note
//! 5. Queries the full state
//! 6. Prints the snapshot

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::protocol::Message};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let port = match args.iter().position(|a| a == "--port") {
        Some(i) => args
            .get(i + 1)
            .ok_or("missing --port value")?
            .parse::<u16>()?,
        None => return Err("usage: agent_session --port PORT".into()),
    };

    let url = format!("ws://127.0.0.1:{}", port);
    println!("Connecting to {}", url);

    let (ws_stream, _) = connect_async(&url).await?;
    let (mut write, mut read) = ws_stream.split();

    // Helper to send a request and await the response
    async fn send_recv(
        write: &mut futures_util::stream::SplitSink<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
            Message,
        >,
        read: &mut futures_util::stream::SplitStream<
            tokio_tungstenite::WebSocketStream<
                tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
            >,
        >,
        req: serde_json::Value,
    ) -> Result<serde_json::Value, Box<dyn std::error::Error>> {
        let req_str = req.to_string();
        write.send(Message::Text(req_str)).await?;

        // Read response - wait for next text message, skipping unsolicited
        // server frames (the connect banner and any async events).
        while let Some(msg) = read.next().await {
            match msg? {
                Message::Text(text) => {
                    let value: serde_json::Value = serde_json::from_str(&text)?;
                    if value["type"] == "hello" || value["type"] == "event" {
                        continue;
                    }
                    return Ok(value);
                }
                Message::Ping(_) => continue,
                Message::Close(_) => return Err("connection closed".into()),
                _ => continue,
            }
        }
        Err("no response".into())
    }

    // 1. Add a note at the current cursor position
    println!("Adding first note...");
    let resp = send_recv(
        &mut write,
        &mut read,
        json!({
            "type": "run_action",
            "id": 1,
            "action": "add_note",
            "params": {}
        }),
    )
    .await?;
    println!("Response: {}", serde_json::to_string_pretty(&resp)?);

    // Small delay to ensure server is ready for next request
    tokio::time::sleep(Duration::from_millis(100)).await;

    // 2. Move cursor right
    println!("Moving cursor right...");
    let resp = send_recv(
        &mut write,
        &mut read,
        json!({
            "type": "run_action",
            "id": 2,
            "action": "cursor_right",
            "params": {}
        }),
    )
    .await?;
    println!("Response: {}", serde_json::to_string_pretty(&resp)?);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 3. Add another note
    println!("Adding second note...");
    let resp = send_recv(
        &mut write,
        &mut read,
        json!({
            "type": "run_action",
            "id": 3,
            "action": "add_note",
            "params": {}
        }),
    )
    .await?;
    println!("Response: {}", serde_json::to_string_pretty(&resp)?);

    tokio::time::sleep(Duration::from_millis(100)).await;

    // 4. Query the full state
    println!("Querying state...");
    let resp = send_recv(
        &mut write,
        &mut read,
        json!({
            "type": "query",
            "id": 4,
            "what": "state"
        }),
    )
    .await?;
    println!("State snapshot:");
    println!("{}", serde_json::to_string_pretty(&resp)?);

    // 5. Query available actions
    println!("\nQuerying available actions...");
    let resp = send_recv(
        &mut write,
        &mut read,
        json!({
            "type": "query",
            "id": 5,
            "what": "actions"
        }),
    )
    .await?;
    if let Some(actions) = resp.get("actions").and_then(|v| v.as_array()) {
        println!("Available actions ({}):", actions.len());
        for action in actions {
            if let Some(name) = action.as_str() {
                println!("  - {}", name);
            }
        }
    }

    // Close the connection
    write.close().await?;

    println!("\nDone!");
    Ok(())
}
