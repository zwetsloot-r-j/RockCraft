//! Channel-routed control server (M4-F variant of [`ControlServer`]).
//!
//! Where [`crate::server::ControlServer`] owns the [`Composer`] behind an
//! `Arc<Mutex<…>>` and applies requests itself, [`CommandServer`] owns **no
//! composer at all**. Each parsed [`Request`] is forwarded to the application
//! over an `mpsc` channel as a [`RemoteCommand`], and the socket task awaits the
//! [`Response`] on a paired `oneshot`. This lets the live TUI keep a single,
//! sync-owned `Composer` (inside its edit screen) that both the keyboard and the
//! remote agent mutate, without the socket task ever touching the composer or
//! the render/MIDI loop ever awaiting on the socket.
//!
//! Loopback-only and unauthenticated, exactly like [`ControlServer`] — see that
//! module's note. Parse errors are answered directly by the socket task (never
//! routed through the channel), so the app loop only ever sees valid requests.
//!
//! [`Composer`]: rockcraft_core::Composer
//! [`ControlServer`]: crate::server::ControlServer

use std::future::Future;
use std::io;
use std::net::SocketAddr;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use crate::protocol::{hello, Request, Response};

/// A control request handed from the socket task to the application loop, paired
/// with a one-shot channel for the application to return its [`Response`].
///
/// The app drains a [`mpsc::Receiver<RemoteCommand>`] once per loop iteration,
/// applies each `req` against the composer it owns, and replies over `reply`.
pub struct RemoteCommand {
    /// The parsed request to apply against the application's composer.
    pub req: Request,
    /// Where the application sends the response once it has handled `req`.
    pub reply: oneshot::Sender<Response>,
}

/// A localhost-only WebSocket control server that routes requests to the
/// application over a channel instead of owning the composer.
///
/// Construct with [`CommandServer::bind`] (passing the `mpsc::Sender` half of
/// the command channel), read the bound port back with
/// [`CommandServer::local_addr`], then drive it with [`CommandServer::serve`] or
/// [`CommandServer::serve_with_shutdown`].
pub struct CommandServer {
    listener: TcpListener,
    local_addr: SocketAddr,
    commands: mpsc::Sender<RemoteCommand>,
}

impl CommandServer {
    /// Bind to `addr` and forward every request over `commands`.
    ///
    /// Pass `"127.0.0.1:0"` to let the OS assign a port; read the actual address
    /// back with [`local_addr`](Self::local_addr). Refuses any non-loopback
    /// address with [`io::ErrorKind::InvalidInput`]: this is a loopback-only
    /// control channel with no authentication.
    pub async fn bind(addr: &str, commands: mpsc::Sender<RemoteCommand>) -> io::Result<Self> {
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
            commands,
        })
    }

    /// The actually-bound local address (the port may have been OS-assigned).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// Accept connections forever, handling each on its own task.
    pub async fn serve(self) -> io::Result<()> {
        self.serve_with_shutdown(std::future::pending::<()>()).await
    }

    /// Accept connections until `shutdown` resolves, then return cleanly.
    ///
    /// In-flight connection tasks are detached and are not awaited.
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
                    let commands = self.commands.clone();
                    tokio::spawn(async move {
                        // Connection-level errors are non-fatal to the server.
                        let _ = handle_connection(stream, commands).await;
                    });
                }
            }
        }
    }
}

/// Run the WebSocket handshake, then service text frames until the peer closes.
async fn handle_connection(
    stream: TcpStream,
    commands: mpsc::Sender<RemoteCommand>,
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
                let response = process_text(&commands, &text).await;
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

/// Parse one text frame into a [`Request`], forward it to the application, and
/// await the reply.
///
/// On parse failure, answers directly with `bad_request: …` (never routes a
/// malformed frame through the channel). If the application has shut its
/// receiver, or drops the reply without answering, the socket task synthesises
/// an error response rather than hanging the connection.
async fn process_text(commands: &mpsc::Sender<RemoteCommand>, text: &str) -> Response {
    let req = match serde_json::from_str::<Request>(text) {
        Ok(req) => req,
        Err(e) => {
            return Response::Err {
                id: None,
                error: format!("bad_request: {e}"),
            }
        }
    };
    let id = req.id();
    let (reply_tx, reply_rx) = oneshot::channel();
    if commands
        .send(RemoteCommand {
            req,
            reply: reply_tx,
        })
        .await
        .is_err()
    {
        return Response::Err {
            id,
            error: "unavailable: control channel closed".into(),
        };
    }
    match reply_rx.await {
        Ok(response) => response,
        Err(_) => Response::Err {
            id,
            error: "unavailable: no reply from application".into(),
        },
    }
}
