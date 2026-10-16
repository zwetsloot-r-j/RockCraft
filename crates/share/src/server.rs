//! The LAN share server (M26-B): one per RockCraft instance, serving its
//! library to the home network.
//!
//! Runs on its **own** thread with its own tokio runtime, so it keeps serving
//! whatever screen the app shows and never touches the UI or MIDI threads.
//! Every request from a peer outside the home network is refused (403); there
//! is no other auth, by decision (see `docs/SHARING.md`).

use std::io::{Seek, SeekFrom};
use std::net::{SocketAddr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::body::Body;
use axum::extract::{ConnectInfo, DefaultBodyLimit, Path as UrlPath, Query, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::oneshot;

use crate::archive::{self, ArchiveError};
use crate::error::ShareError;
use crate::net::is_home_network;

/// The port sharing tries first; the next nine are fallbacks when it's busy.
/// Kept below the OS ephemeral ranges (Linux 32768+, Windows 49152+): WSL2 in
/// mirrored mode reserves Linux's ephemeral range on the Windows side, and
/// 47800 fell inside it there, so every bind failed.
pub const DEFAULT_PORT: u16 = 27800;
/// How many ports from [`DEFAULT_PORT`] up to try.
const PORT_TRIES: u16 = 10;
/// HTTP API version, served by `/api/v1/info`.
pub const API_VERSION: u32 = 1;
/// Cap on an uploaded archive (4 GiB), matching the unpack limit.
const MAX_UPLOAD_BYTES: u64 = 4 << 30;

/// `GET /api/v1/info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    /// Always `"rockcraft"`.
    pub app: String,
    /// [`API_VERSION`].
    pub api: u32,
    /// The sharing machine's name.
    pub name: String,
    /// Number of songs it shares.
    pub songs: usize,
}

/// One song in `GET /api/v1/songs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RemoteSong {
    /// `<root label>~<folder name>`, e.g. `library~frog-song`.
    pub id: String,
    /// The bundle folder name.
    pub name: String,
    /// `recorded` / `composed` / `edited` / `imported`, or null.
    pub origin: Option<String>,
    pub note_count: usize,
    pub duration_us: u64,
    pub has_backing: bool,
    pub has_video: bool,
    /// Total size of the files that travel (uncompressed).
    pub size_bytes: u64,
    /// [`archive::chart_hash`]: tells "same song" from "same name".
    pub chart_hash: String,
}

/// What every handler can see.
struct Ctx {
    roots: Vec<PathBuf>,
    library: PathBuf,
    name: String,
}

/// A running share server. Dropping it (or [`stop`](Self::stop)) stops
/// accepting connections at once; transfers already running finish on the
/// server thread, which then exits.
pub struct ShareServer {
    port: u16,
    shutdown: Option<oneshot::Sender<()>>,
}

impl ShareServer {
    /// Share `roots` (listed and downloadable) on `0.0.0.0:<port>`, with
    /// uploads landing in `library` under the machine name `name`. `port`
    /// `None` tries [`DEFAULT_PORT`] and the nine above it, then any free
    /// port (discovery advertises whichever it got); `Some(0)` takes any free
    /// port (tests).
    pub fn start(
        port: Option<u16>,
        roots: Vec<PathBuf>,
        library: PathBuf,
        name: String,
    ) -> Result<ShareServer, ShareError> {
        let listener = bind(port)?;
        let port = listener.local_addr()?.port();
        listener.set_nonblocking(true)?;
        let (tx, rx) = oneshot::channel::<()>();
        let ctx = Arc::new(Ctx {
            roots,
            library,
            name,
        });
        std::thread::Builder::new()
            .name("rockcraft-share".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .enable_all()
                    .build()
                {
                    Ok(rt) => rt,
                    Err(e) => {
                        eprintln!("[share] runtime failed: {e}");
                        return;
                    }
                };
                runtime.block_on(async move {
                    let listener = match tokio::net::TcpListener::from_std(listener) {
                        Ok(l) => l,
                        Err(e) => {
                            eprintln!("[share] listener failed: {e}");
                            return;
                        }
                    };
                    let app = router(ctx).into_make_service_with_connect_info::<SocketAddr>();
                    let served = axum::serve(listener, app)
                        .with_graceful_shutdown(async {
                            let _ = rx.await;
                        })
                        .await;
                    if let Err(e) = served {
                        eprintln!("[share] server stopped: {e}");
                    }
                });
            })?;
        eprintln!("[share] sharing on port {port}");
        Ok(ShareServer {
            port,
            shutdown: Some(tx),
        })
    }

    /// The port it listens on.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Stop sharing.
    pub fn stop(self) {}
}

impl Drop for ShareServer {
    fn drop(&mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
    }
}

/// Bind the listening socket (see [`ShareServer::start`] for `port`).
fn bind(port: Option<u16>) -> Result<TcpListener, ShareError> {
    let ports: Vec<u16> = match port {
        Some(p) => vec![p],
        // Last resort 0: the OS picks a free port.
        None => (DEFAULT_PORT..DEFAULT_PORT + PORT_TRIES)
            .chain([0])
            .collect(),
    };
    let mut last = String::new();
    for p in ports {
        match TcpListener::bind(("0.0.0.0", p)) {
            Ok(l) => return Ok(l),
            Err(e) => last = format!("port {p}: {e}"),
        }
    }
    Err(ShareError::Bind(last))
}

fn router(ctx: Arc<Ctx>) -> Router {
    Router::new()
        .route("/api/v1/info", get(info))
        .route("/api/v1/songs", get(songs))
        .route("/api/v1/songs/{id}/archive", get(download))
        .route("/api/v1/songs/{slug}", put(upload))
        .layer(DefaultBodyLimit::disable())
        .layer(middleware::from_fn(guard))
        .with_state(ctx)
}

/// Refuse peers outside the home network; log every request.
async fn guard(ConnectInfo(peer): ConnectInfo<SocketAddr>, req: Request, next: Next) -> Response {
    let line = format!("{} {} from {}", req.method(), req.uri().path(), peer.ip());
    let response = if is_home_network(peer.ip()) {
        next.run(req).await
    } else {
        error(StatusCode::FORBIDDEN, "home network only")
    };
    eprintln!("[share] {line} → {}", response.status().as_u16());
    response
}

/// A JSON `{ "error": msg }` response.
fn error(code: StatusCode, msg: &str) -> Response {
    (code, Json(serde_json::json!({ "error": msg }))).into_response()
}

/// The label a scan root goes by in song ids (its folder name).
fn root_label(root: &Path) -> String {
    root.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "library".to_string())
}

/// Every shareable song under `roots` (bundles that pack cleanly).
pub(crate) fn list_songs(roots: &[PathBuf]) -> Vec<RemoteSong> {
    let mut songs = Vec::new();
    for root in roots {
        let label = root_label(root);
        for entry in rockcraft_midi::bundle::list_library(std::slice::from_ref(root)) {
            let Ok(files) = archive::bundle_files(&entry.dir) else {
                continue;
            };
            let Ok(chart_hash) = archive::chart_hash(&entry.dir) else {
                continue;
            };
            let size_bytes = files
                .iter()
                .filter_map(|f| std::fs::metadata(entry.dir.join(f)).ok())
                .map(|m| m.len())
                .sum();
            let has_video = std::fs::read_to_string(entry.dir.join("meta.json"))
                .ok()
                .and_then(|j| rockcraft_core::RecordingMeta::from_json(&j).ok())
                .is_some_and(|m| m.video.is_some());
            songs.push(RemoteSong {
                id: format!("{label}~{}", entry.name),
                name: entry.name,
                origin: entry.origin.map(|o| o.label().to_string()),
                note_count: entry.note_count,
                duration_us: entry.duration_us,
                has_backing: entry.has_backing,
                has_video,
                size_bytes,
                chart_hash,
            });
        }
    }
    songs.sort_by(|a, b| a.name.cmp(&b.name).then_with(|| a.id.cmp(&b.id)));
    songs
}

/// The bundle folder a song id names, if it exists.
fn find_song(roots: &[PathBuf], id: &str) -> Option<PathBuf> {
    let (label, name) = id.split_once('~')?;
    if name.is_empty() || name == ".." || name.contains(['/', '\\', ':']) {
        return None;
    }
    roots
        .iter()
        .filter(|r| root_label(r) == label)
        .map(|r| r.join(name))
        .find(|dir| dir.join("song.mid").is_file())
}

async fn info(State(ctx): State<Arc<Ctx>>) -> Response {
    let roots = ctx.roots.clone();
    let songs = tokio::task::spawn_blocking(move || rockcraft_midi::bundle::list_library(&roots))
        .await
        .map(|l| l.len())
        .unwrap_or(0);
    Json(ServerInfo {
        app: "rockcraft".into(),
        api: API_VERSION,
        name: ctx.name.clone(),
        songs,
    })
    .into_response()
}

async fn songs(State(ctx): State<Arc<Ctx>>) -> Response {
    let roots = ctx.roots.clone();
    match tokio::task::spawn_blocking(move || list_songs(&roots)).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

/// `GET /songs/{id}/archive`: pack into an anonymous temp file, then stream it.
async fn download(State(ctx): State<Arc<Ctx>>, UrlPath(id): UrlPath<String>) -> Response {
    let roots = ctx.roots.clone();
    let lookup = id.clone();
    let packed = tokio::task::spawn_blocking(move || {
        let dir = find_song(&roots, &lookup)?;
        let name = dir.file_name()?.to_string_lossy().into_owned();
        let packed = (|| -> Result<(std::fs::File, u64), ArchiveError> {
            let mut file = tempfile::tempfile()?;
            archive::pack(&dir, &mut file)?;
            let len = file.seek(SeekFrom::End(0))?;
            file.seek(SeekFrom::Start(0))?;
            Ok((file, len))
        })();
        Some((name, packed))
    })
    .await;
    let (name, (file, len)) = match packed {
        Ok(Some((name, Ok(packed)))) => (name, packed),
        Ok(Some((_, Err(e)))) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Ok(None) => return error(StatusCode::NOT_FOUND, &format!("no song {id:?}")),
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let ascii: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .collect();
    let stream = tokio_util::io::ReaderStream::new(tokio::fs::File::from_std(file));
    (
        [
            (header::CONTENT_TYPE, "application/zip".to_string()),
            (header::CONTENT_LENGTH, len.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"{ascii}.{}\"", archive::ARCHIVE_EXT),
            ),
        ],
        Body::from_stream(stream),
    )
        .into_response()
}

#[derive(Deserialize)]
struct UploadQuery {
    overwrite: Option<String>,
}

/// `PUT /songs/{slug}`: stream the body to a temp file, then unpack it into
/// the library as `slug`.
async fn upload(
    State(ctx): State<Arc<Ctx>>,
    UrlPath(slug): UrlPath<String>,
    Query(q): Query<UploadQuery>,
    body: Body,
) -> Response {
    if slug.is_empty() || rockcraft_midi::bundle::slug(&slug) != slug {
        return error(StatusCode::BAD_REQUEST, "invalid song name");
    }
    let overwrite = matches!(q.overwrite.as_deref(), Some("1" | "true"));
    let file = match tempfile::tempfile() {
        Ok(f) => f,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    };
    let mut out = tokio::fs::File::from_std(file);
    let mut stream = body.into_data_stream();
    let mut total = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = match chunk {
            Ok(c) => c,
            Err(e) => return error(StatusCode::BAD_REQUEST, &e.to_string()),
        };
        total += chunk.len() as u64;
        if total > MAX_UPLOAD_BYTES {
            return error(StatusCode::PAYLOAD_TOO_LARGE, "song is over 4 GiB");
        }
        if let Err(e) = out.write_all(&chunk).await {
            return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
        }
    }
    if let Err(e) = out.flush().await {
        return error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string());
    }
    let file = out.into_std().await;
    let library = ctx.library.clone();
    let name = slug.clone();
    let result = tokio::task::spawn_blocking(move || {
        archive::import_song_archive(file, &library, overwrite, Some(&name))
    })
    .await;
    match result {
        Ok(Ok(_)) => (
            StatusCode::CREATED,
            Json(serde_json::json!({ "id": format!("{}~{slug}", root_label(&ctx.library)) })),
        )
            .into_response(),
        Ok(Err(ArchiveError::Exists(_))) => error(StatusCode::CONFLICT, "exists"),
        Ok(Err(ArchiveError::Io(e))) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
        Ok(Err(e)) => error(StatusCode::BAD_REQUEST, &e.to_string()),
        Err(e) => error(StatusCode::INTERNAL_SERVER_ERROR, &e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bind_falls_back_to_any_port_when_the_range_is_taken() {
        // Hold every default port we can; ones we can't are taken already.
        let _held: Vec<TcpListener> = (DEFAULT_PORT..DEFAULT_PORT + PORT_TRIES)
            .filter_map(|p| TcpListener::bind(("0.0.0.0", p)).ok())
            .collect();
        let listener = bind(None).expect("falls back to an OS-picked port");
        let port = listener.local_addr().unwrap().port();
        assert!(!(DEFAULT_PORT..DEFAULT_PORT + PORT_TRIES).contains(&port));
    }

    #[test]
    fn a_pinned_port_does_not_fall_back() {
        let held = TcpListener::bind(("0.0.0.0", 0)).unwrap();
        let port = held.local_addr().unwrap().port();
        assert!(matches!(bind(Some(port)), Err(ShareError::Bind(_))));
    }
}
