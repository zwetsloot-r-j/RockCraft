//! Integration tests for the channel-routed [`CommandServer`] over a real
//! loopback socket. A stand-in "application" task drains the command channel and
//! applies each request against a `Composer` it owns — exactly the seam the TUI
//! run loop fills in production.

#[cfg(test)]
mod tests {
    use futures_util::{SinkExt, StreamExt};
    use serde_json::Value;
    use tokio::sync::mpsc;
    use tokio_tungstenite::tungstenite::Message;

    use crate::command::{CommandServer, RemoteCommand};
    use crate::protocol::handle;
    use rockcraft_core::Composer;

    /// Spawn a `CommandServer` plus a stand-in application loop that owns a
    /// single `Composer` and answers every `RemoteCommand` via `handle`.
    /// Returns the `ws://` URL and the two task handles to abort at test end.
    async fn spawn_server() -> (
        String,
        tokio::task::JoinHandle<()>,
        tokio::task::JoinHandle<()>,
    ) {
        let (tx, mut rx) = mpsc::channel::<RemoteCommand>(32);
        let server = CommandServer::bind("127.0.0.1:0", tx)
            .await
            .expect("bind loopback");
        let url = format!("ws://{}", server.local_addr());

        let server_handle = tokio::spawn(async move {
            let _ = server.serve().await;
        });

        // The application: one Composer, drained in receive order, never shared.
        let app_handle = tokio::spawn(async move {
            let mut composer = Composer::new();
            while let Some(cmd) = rx.recv().await {
                let response = handle(&mut composer, cmd.req);
                let _ = cmd.reply.send(response);
            }
        });

        (url, server_handle, app_handle)
    }

    async fn connect(
        url: &str,
    ) -> tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>
    {
        let (ws, _resp) = tokio_tungstenite::connect_async(url)
            .await
            .expect("client connects");
        ws
    }

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
                _ => continue,
            }
        }
    }

    #[tokio::test]
    async fn bind_rejects_non_loopback() {
        let (tx, _rx) = mpsc::channel::<RemoteCommand>(1);
        match CommandServer::bind("0.0.0.0:0", tx).await {
            Ok(_) => panic!("non-loopback bind must be refused"),
            Err(e) => assert_eq!(e.kind(), std::io::ErrorKind::InvalidInput),
        }
    }

    #[tokio::test]
    async fn run_action_routed_through_channel_applies_to_app_composer() {
        let (url, server_handle, app_handle) = spawn_server().await;
        let mut ws = connect(&url).await;

        let reply = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        assert_eq!(reply["type"], "ok");
        assert_eq!(reply["state"]["notes"].as_array().unwrap().len(), 1);

        // The id round-trips, proving the oneshot carried the app's own response.
        let reply = round_trip(
            &mut ws,
            r#"{"type":"run_action","id":7,"action":"cursor_right"}"#,
        )
        .await;
        assert_eq!(reply["id"], 7);

        server_handle.abort();
        app_handle.abort();
    }

    #[tokio::test]
    async fn malformed_json_answered_without_touching_channel() {
        let (url, server_handle, app_handle) = spawn_server().await;
        let mut ws = connect(&url).await;

        let reply = round_trip(&mut ws, "not json {").await;
        assert_eq!(reply["type"], "err");
        assert!(reply["error"].as_str().unwrap().starts_with("bad_request:"));

        // The socket stays open and the next valid request still round-trips.
        let ok = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        assert_eq!(ok["type"], "ok");

        server_handle.abort();
        app_handle.abort();
    }

    #[tokio::test]
    async fn serve_with_shutdown_stops_cleanly() {
        let (tx, _rx) = mpsc::channel::<RemoteCommand>(1);
        let server = CommandServer::bind("127.0.0.1:0", tx).await.expect("bind");
        let (sd_tx, sd_rx) = tokio::sync::oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            server
                .serve_with_shutdown(async {
                    let _ = sd_rx.await;
                })
                .await
        });

        sd_tx.send(()).expect("send shutdown");
        let result = task.await.expect("task joins");
        assert!(result.is_ok(), "graceful shutdown returns Ok");
    }

    #[tokio::test]
    async fn app_dropping_receiver_yields_unavailable_error() {
        // Bind a server whose command receiver is dropped immediately: the socket
        // task must answer `unavailable: …` instead of hanging the connection.
        let (tx, rx) = mpsc::channel::<RemoteCommand>(1);
        drop(rx);
        let server = CommandServer::bind("127.0.0.1:0", tx).await.expect("bind");
        let url = format!("ws://{}", server.local_addr());
        let server_handle = tokio::spawn(async move {
            let _ = server.serve().await;
        });

        let mut ws = connect(&url).await;
        let reply = round_trip(&mut ws, r#"{"type":"run_action","action":"add_note"}"#).await;
        assert_eq!(reply["type"], "err");
        assert!(reply["error"].as_str().unwrap().starts_with("unavailable:"));

        server_handle.abort();
    }
}
