//! Integration tests for the control server over a real loopback socket.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio::sync::Mutex;
    use tokio_tungstenite::tungstenite::Message;

    use crate::server::ControlServer;
    use rockcraft_core::Composer;

    /// Bind a server on an OS-assigned loopback port, run it on a task, and
    /// return its `ws://` URL plus a handle to abort it when the test ends.
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

    /// Connect a websocket client to `url`.
    async fn connect(
        url: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let (ws, _resp) = tokio_tungstenite::connect_async(url)
            .await
            .expect("client connects");
        ws
    }

    /// Send `text`, await the next text frame, and parse it as JSON.
    async fn round_trip<S>(ws: &mut S, text: &str) -> Value
    where
        S: SinkExt<Message, Error = tokio_tungstenite::tungstenite::Error>
            + StreamExt<Item = Result<Message, tokio_tungstenite::tungstenite::Error>>
            + Unpin,
    {
        ws.send(Message::Text(text.to_string()))
            .await
            .expect("send");
        loop {
            match ws.next().await.expect("a reply").expect("no ws error") {
                Message::Text(t) => return serde_json::from_str(&t).expect("reply is json"),
                // Ignore any control frames the server might interleave.
                _ => continue,
            }
        }
    }

    #[tokio::test]
    async fn bind_rejects_non_loopback() {
        let composer = Arc::new(Mutex::new(Composer::new()));
        // 0.0.0.0 binds to all interfaces — not loopback — and must be refused.
        match ControlServer::bind("0.0.0.0:0", composer).await {
            Ok(_) => panic!("non-loopback bind must be refused"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput),
        }
    }

    #[tokio::test]
    async fn run_action_add_note_returns_ok_with_one_note() {
        let (url, handle) = spawn_server().await;
        let mut ws = connect(&url).await;

        let reply = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        assert_eq!(reply["type"], "ok");
        assert_eq!(reply["state"]["notes"].as_array().unwrap().len(), 1);

        handle.abort();
    }

    #[tokio::test]
    async fn second_note_at_moved_cursor_then_query_matches() {
        let (url, handle) = spawn_server().await;
        let mut ws = connect(&url).await;

        round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        // Move the cursor so the next note lands in a fresh cell (add_note on an
        // occupied cell replaces rather than adds).
        round_trip(&mut ws, r#"{"type":"run_action","action":"cursor_right"}"#).await;
        let second = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        assert_eq!(second["type"], "ok");
        assert_eq!(second["state"]["notes"].as_array().unwrap().len(), 2);

        // A separate state query must report the same snapshot.
        let queried = round_trip(&mut ws, r#"{"type":"query","what":"State"}"#).await;
        assert_eq!(queried["type"], "ok");
        assert_eq!(queried["state"], second["state"]);

        handle.abort();
    }

    #[tokio::test]
    async fn malformed_json_returns_bad_request_and_keeps_socket_open() {
        let (url, handle) = spawn_server().await;
        let mut ws = connect(&url).await;

        let reply = round_trip(&mut ws, "this is not json {").await;
        assert_eq!(reply["type"], "err");
        assert!(
            reply["error"].as_str().unwrap().starts_with("bad_request:"),
            "expected bad_request prefix, got {:?}",
            reply["error"]
        );
        assert!(reply["id"].is_null());

        // Socket stays open: a follow-up request still succeeds.
        let ok = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        assert_eq!(ok["type"], "ok");

        handle.abort();
    }

    #[tokio::test]
    async fn two_sequential_clients_share_composer_state() {
        let (url, handle) = spawn_server().await;

        // First client adds a note, then disconnects.
        {
            let mut ws = connect(&url).await;
            let reply = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
            assert_eq!(reply["state"]["notes"].as_array().unwrap().len(), 1);
            ws.close(None).await.ok();
        }

        // Second client sees the note placed by the first.
        {
            let mut ws = connect(&url).await;
            let reply = round_trip(&mut ws, r#"{"type":"query","what":"State"}"#).await;
            assert_eq!(reply["type"], "ok");
            assert_eq!(reply["state"]["notes"].as_array().unwrap().len(), 1);
        }

        handle.abort();
    }

    #[tokio::test]
    async fn serve_with_shutdown_stops_cleanly() {
        let composer = Arc::new(Mutex::new(Composer::new()));
        let server = ControlServer::bind("127.0.0.1:0", composer)
            .await
            .expect("bind");
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async {
                    let _ = rx.await;
                })
                .await
        });

        tx.send(()).expect("send shutdown");
        let result = task.await.expect("task joins");
        assert!(result.is_ok(), "graceful shutdown returns Ok");
    }
}
