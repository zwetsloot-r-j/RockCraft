//! RockCraft terminal frontend entry point.
//!
//! Thin binary wrapper — the app logic lives in `lib.rs` (the library target
//! of this crate) so integration tests in `tests/` can access it.

use std::net::SocketAddr;

use rockcraft_audio::AudioOut;
use rockcraft_control::{CommandServer, RemoteCommand};
use rockcraft_midi::{LiveInput, MockKeyboard, NoteSource};
use rockcraft_tui::app;
use tokio::sync::{mpsc, oneshot};

/// Default control-server bind address: an OS-assigned loopback port. Override
/// with `ROCKCRAFT_CONTROL_ADDR` (must stay loopback — the server refuses other
/// interfaces).
const DEFAULT_CONTROL_ADDR: &str = "127.0.0.1:0";

/// Capacity of the socket→app command channel. Small: requests are applied
/// within a frame, so this only buffers a brief burst.
const CONTROL_CHANNEL_CAP: usize = 64;

/// A running control server: its command receiver (handed to the app loop) plus
/// the shutdown handle and thread join for a clean stop on quit.
struct ControlServerHandle {
    rx: mpsc::Receiver<RemoteCommand>,
    shutdown: oneshot::Sender<()>,
    thread: std::thread::JoinHandle<()>,
}

fn main() {
    // Args: `--mock` forces the keyboard mock; `--edit` boots straight into the
    // composer; `--control` starts the localhost control server; the first
    // non-flag arg is the MIDI port-name substring (default "casio").
    let args: Vec<String> = std::env::args().skip(1).collect();
    let force_mock = args.iter().any(|a| a == "--mock");
    let start_edit = args.iter().any(|a| a == "--edit");
    // The control server starts when `--control` is passed or the bind address
    // env var is set (the latter doubles as the opt-in for scripted agents).
    let control_enabled =
        args.iter().any(|a| a == "--control") || std::env::var("ROCKCRAFT_CONTROL_ADDR").is_ok();
    let filter = args
        .iter()
        .find(|a| !a.starts_with("--"))
        .cloned()
        .unwrap_or_else(|| "casio".to_string());
    // Optional second arg: path to a backing audio file (wav/mp3/ogg/flac).
    // When provided, entering Record plays it alongside your performance.
    let backing_path = std::env::args().nth(2).map(std::path::PathBuf::from);

    // Source selection: explicit `--mock`, otherwise the live piano — and when
    // no port matches, fall back to the mock so the app always launches.
    let input: Box<dyn NoteSource> = if force_mock {
        eprintln!("Using MockKeyboard (--mock): type the home/QWERTY rows to play notes.");
        Box::new(MockKeyboard::new())
    } else {
        match LiveInput::connect(&filter) {
            Ok(i) => Box::new(i),
            Err(e) => {
                eprintln!(
                    "No MIDI input ({e}); falling back to MockKeyboard — type to play notes."
                );
                Box::new(MockKeyboard::new())
            }
        }
    };

    // Audio is optional: if there's no SoundFont / output device, run silently.
    // `audio` is bound for the whole run so the output stream stays open.
    let audio = match AudioOut::new() {
        Ok(a) => Some(a),
        Err(e) => {
            eprintln!("Audio disabled: {e}");
            None
        }
    };
    let synth = audio.as_ref().map(AudioOut::synth);

    // Start the control server on its own thread (tokio stays off the terminal/
    // MIDI loop). `None` keeps normal runs socket-free.
    let control = if control_enabled {
        match start_control_server() {
            Ok(handle) => Some(handle),
            Err(e) => {
                eprintln!("Control server disabled: {e}");
                None
            }
        }
    } else {
        None
    };
    let (commands, shutdown) = match control {
        Some(ControlServerHandle {
            rx,
            shutdown,
            thread,
        }) => (Some(rx), Some((shutdown, thread))),
        None => (None, None),
    };

    let result = app::run(input, synth, backing_path, start_edit, commands);

    // Signal the server to stop and join its thread before exiting.
    if let Some((shutdown, thread)) = shutdown {
        let _ = shutdown.send(());
        let _ = thread.join();
    }

    if let Err(e) = result {
        eprintln!("TUI error: {e}");
        std::process::exit(1);
    }
}

/// Spawn a tokio runtime on a dedicated thread, bind the [`CommandServer`], and
/// serve until shutdown. Blocks only until the bind result is known so the
/// caller can report the port (or the failure) synchronously.
fn start_control_server() -> std::io::Result<ControlServerHandle> {
    let addr = std::env::var("ROCKCRAFT_CONTROL_ADDR")
        .unwrap_or_else(|_| DEFAULT_CONTROL_ADDR.to_string());
    let (cmd_tx, cmd_rx) = mpsc::channel::<RemoteCommand>(CONTROL_CHANNEL_CAP);
    let (shutdown_tx, shutdown_rx) = oneshot::channel::<()>();
    // The bind result (the port, or the error) is reported back synchronously so
    // `main` can print the address before the TUI takes over the terminal.
    let (bound_tx, bound_rx) = std::sync::mpsc::channel::<std::io::Result<SocketAddr>>();

    let thread = std::thread::Builder::new()
        .name("rockcraft-control".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    let _ = bound_tx.send(Err(e));
                    return;
                }
            };
            rt.block_on(async move {
                match CommandServer::bind(&addr, cmd_tx).await {
                    Ok(server) => {
                        let _ = bound_tx.send(Ok(server.local_addr()));
                        let _ = server
                            .serve_with_shutdown(async {
                                let _ = shutdown_rx.await;
                            })
                            .await;
                    }
                    Err(e) => {
                        let _ = bound_tx.send(Err(e));
                    }
                }
            });
        })?;

    match bound_rx.recv() {
        Ok(Ok(addr)) => {
            eprintln!("Control server listening on ws://{addr}");
            Ok(ControlServerHandle {
                rx: cmd_rx,
                shutdown: shutdown_tx,
                thread,
            })
        }
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            let _ = thread.join();
            Err(std::io::Error::other(
                "control server thread exited before binding",
            ))
        }
    }
}
