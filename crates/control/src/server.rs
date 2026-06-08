//! Localhost WebSocket control server (M4-E).
//!
//! Wraps the M4-D protocol (`Request` / `handle` / `Response`) in a running
//! `tokio` + `tokio-tungstenite` server. Each text frame is parsed into a
//! [`Request`], applied through [`handle`] against a shared [`Composer`], and
//! the resulting [`Response`] is written back as a text frame.
//!
//! **No authentication or TLS, by design.** This is a development/test control
//! channel and is bound to loopback only — [`ControlServer::bind`] refuses any
//! non-loopback address. Do not expose it on a public interface.
//!
//! The composer is the single mutable resource and is serialised behind an
//! `Arc<Mutex<Composer>>`. The lock is held only for the synchronous duration of
//! [`handle`], never across an `.await` that waits on socket I/O.

use std::future::Future;
use std::io;
use std::net::SocketAddr;
use std::sync::Arc;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::Mutex;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use rockcraft_core::Composer;

use crate::protocol::{handle, hello, Request, Response};

/// A localhost-only WebSocket control server over the M4-D protocol.
///
/// Bind with [`ControlServer::bind`] (callers pass `"127.0.0.1:0"` to get an
/// OS-assigned port, then read it back via [`ControlServer::local_addr`]), then
/// drive it with [`ControlServer::serve`] (run forever) or
/// [`ControlServer::serve_with_shutdown`] (run until a shutdown future resolves).
pub struct ControlServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    composer: Arc<Mutex<Composer>>,
}

impl ControlServer {
    /// Bind to `addr` and share `composer` across all connections.
    ///
    /// Pass `"127.0.0.1:0"` to let the OS assign a port; read the actual address
    /// back with [`local_addr`](Self::local_addr). Refuses any non-loopback
    /// address with [`io::ErrorKind::InvalidInput`]: this is a loopback-only
    /// control channel with no authentication.
    pub async fn bind(addr: &str, composer: Arc<Mutex<Composer>>) -> io::Result<Self> {
        let listener = TcpListener::bind(addr).await?;
        let local_addr = listener.local_addr()?;
        if !local_addr.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("refusing non-loopback bind: {local_addr}"),
            ));
        }
        Ok(Self {
            listener,
            local_addr,
            composer,
        })
    }

    /// The actually-bound local address (the port may have been OS-assigned).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accept connections forever, handling each on its own task.
    ///
    /// To stop the server, drive it on a [`tokio::task`] and abort the handle,
    /// or use [`serve_with_shutdown`](Self::serve_with_shutdown) for a graceful
    /// stop.
    pub async fn serve(self) -> io::Result<()> {
        self.serve_with_shutdown(std::future::pending::<()>()).await
    }

    /// Accept connections until `shutdown` resolves, then return cleanly.
    ///
    /// `shutdown` can be anything that resolves to `()` — a
    /// [`tokio::sync::oneshot`] receiver mapped to `()`, a `CancellationToken`'s
    /// `cancelled()` future, etc. In-flight connection tasks are detached and
    /// are not awaited.
    pub async fn serve_with_shutdown<F>(self, shutdown: F) -> io::Result<()>
    where
        F: Future<Output = ()>,
    {
        tokio::pin!(shutdown);
        loop {
            tokio::select! {
                _ = &mut shutdown => return Ok(()),
                accepted = self.listener.accept() => {
                    let (stream, _peer) = accepted?;
                    let composer = Arc::clone(&self.composer);
                    tokio::spawn(async move {
                        // Connection-level errors are non-fatal to the server.
                        let _ = handle_connection(stream, composer).await;
                    });
                }
            }
        }
    }
}

/// Run the WebSocket handshake, then service text frames until the peer closes.
async fn handle_connection(
    stream: TcpStream,
    composer: Arc<Mutex<Composer>>,
) -> Result<(), WsError> {
    let ws = tokio_tungstenite::accept_async(stream).await?;
    let (mut write, mut read) = ws.split();

    // Greet the client before it sends anything: the banner advertises the
    // protocol verbs and that `query help` enumerates every action.
    let banner = serde_json::to_string(&hello()).expect("hello banner serialises");
    write.send(Message::Text(banner)).await?;

    while let Some(msg) = read.next().await {
        match msg? {
            Message::Text(text) => {
                let response = process_text(&composer, &text).await;
                let payload = serde_json::to_string(&response).unwrap_or_else(|e| {
                    format!(r#"{{"type":"err","id":null,"error":"internal: {e}"}}"#)
                });
                write.send(Message::Text(payload)).await?;
            }
            Message::Ping(payload) => write.send(Message::Pong(payload)).await?,
            Message::Close(_) => break,
            // Binary / Pong / continuation frames are not part of the protocol.
            _ => {}
        }
    }

    Ok(())
}

/// Parse one text frame into a [`Request`] and apply it.
///
/// On parse failure, returns `Response::Err { id: None, error: "bad_request: …" }`
/// rather than dropping the socket. The composer lock is acquired only for the
/// synchronous [`handle`] call and released before the caller awaits the write.
async fn process_text(composer: &Arc<Mutex<Composer>>, text: &str) -> Response {
    match serde_json::from_str::<Request>(text) {
        Ok(req) => {
            let mut c = composer.lock().await;
            handle(&mut c, req)
        }
        Err(e) => Response::Err {
            id: None,
            error: format!("bad_request: {e}"),
        },
    }
}
