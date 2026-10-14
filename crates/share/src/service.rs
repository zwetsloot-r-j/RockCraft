//! [`ShareService`]: the sharing state both frontends hold (M26-B), so each
//! `share_*` host-command arm is a one-liner.
//!
//! It owns the optional running [`ShareServer`] and a table of background
//! transfers. Each download/upload runs on its own thread and reports
//! progress into the table, which `share_transfers` reads.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::archive;
use crate::client;
use crate::error::ShareError;
use crate::net::{host_name, lan_addresses};
use crate::server::{RemoteSong, ShareServer};

/// Finished transfers kept in the table (oldest dropped first).
const MAX_TRANSFERS: usize = 50;

/// `share_status` / `share_start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShareStatus {
    pub running: bool,
    pub port: Option<u16>,
    /// `ip:port` per LAN address while running; empty when stopped.
    pub addresses: Vec<String>,
    pub name: String,
}

/// How a remote song relates to the local library.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalState {
    /// A local bundle has the same chart (`chart_hash`).
    Same,
    /// A local bundle has the same name but another chart.
    Different,
    /// Nothing local matches.
    Absent,
}

/// One row of `share_list`: the remote song plus how it relates to ours.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ListedSong {
    #[serde(flatten)]
    pub song: RemoteSong,
    pub local: LocalState,
}

/// Transfer direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Down,
    Up,
}

/// Where a transfer is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum TransferState {
    Running,
    /// Finished; `dir` is the new local bundle for a download.
    Done {
        dir: Option<String>,
    },
    Failed {
        detail: String,
    },
    /// The target name was taken (and `overwrite` was off).
    Exists {
        slug: String,
    },
}

/// One background download or upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Transfer {
    pub id: u64,
    pub direction: Direction,
    pub server: String,
    pub name: String,
    pub bytes_done: u64,
    pub bytes_total: u64,
    #[serde(flatten)]
    pub state: TransferState,
}

type Table = Arc<Mutex<Vec<Transfer>>>;

/// Sharing state for one app instance.
pub struct ShareService {
    roots: Vec<PathBuf>,
    library: PathBuf,
    server: Option<ShareServer>,
    transfers: Table,
    next_id: AtomicU64,
}

impl ShareService {
    /// Share (and compare against) `roots`; downloads land in `library`.
    pub fn new(roots: Vec<PathBuf>, library: PathBuf) -> Self {
        ShareService {
            roots,
            library,
            server: None,
            transfers: Arc::new(Mutex::new(Vec::new())),
            next_id: AtomicU64::new(1),
        }
    }

    /// The app's default library roots and library folder.
    pub fn with_defaults() -> Self {
        Self::new(
            rockcraft_midi::bundle::default_scan_roots(),
            rockcraft_midi::bundle::library_root(),
        )
    }

    /// Start sharing (see [`ShareServer::start`] for `port`). Already sharing
    /// on that port (or any port, for `None`): a no-op. Another port: restart.
    pub fn start(&mut self, port: Option<u16>) -> Result<ShareStatus, ShareError> {
        let restart = match (&self.server, port) {
            (None, _) => true,
            (Some(s), Some(p)) => p != 0 && s.port() != p,
            (Some(_), None) => false,
        };
        if restart {
            self.server = None;
            self.server = Some(ShareServer::start(
                port,
                self.roots.clone(),
                self.library.clone(),
                host_name(),
            )?);
        }
        Ok(self.status())
    }

    /// Stop sharing (a no-op when not sharing).
    pub fn stop(&mut self) {
        self.server = None;
    }

    /// Whether and where this instance shares.
    pub fn status(&self) -> ShareStatus {
        let port = self.server.as_ref().map(ShareServer::port);
        ShareStatus {
            running: port.is_some(),
            port,
            addresses: port
                .map(|p| {
                    lan_addresses()
                        .iter()
                        .map(|ip| format!("{ip}:{p}"))
                        .collect()
                })
                .unwrap_or_default(),
            name: host_name(),
        }
    }

    /// The songs `server` shares, each classified against the local library.
    pub fn list(&self, server: &str) -> Result<Vec<ListedSong>, ShareError> {
        self.lister().list(server)
    }

    /// A detached [`Lister`], so a frontend can make the (slow, networked)
    /// list call without holding its lock on the service.
    pub fn lister(&self) -> Lister {
        Lister {
            roots: self.roots.clone(),
        }
    }

    /// Start downloading song `id` from `server` in the background (into
    /// `library/<rename_to or name>`). Returns the transfer id.
    pub fn download(
        &self,
        server: &str,
        id: &str,
        overwrite: bool,
        rename_to: Option<String>,
    ) -> u64 {
        let name = id.split_once('~').map_or(id, |(_, n)| n).to_string();
        let tid = self.begin(Direction::Down, server, &name);
        let (table, server, id, library) = (
            self.transfers.clone(),
            server.to_string(),
            id.to_string(),
            self.library.clone(),
        );
        std::thread::spawn(move || {
            let result = client::list(&server).and_then(|songs| {
                let song = songs
                    .into_iter()
                    .find(|s| s.id == id)
                    .ok_or_else(|| ShareError::NotFound(id.clone()))?;
                set_total(&table, tid, song.size_bytes);
                client::download(
                    &server,
                    &song,
                    &library,
                    overwrite,
                    rename_to.as_deref(),
                    &|done, total| set_progress(&table, tid, done, total),
                )
            });
            finish(
                &table,
                tid,
                result.map(|d| Some(d.to_string_lossy().into_owned())),
            );
        });
        tid
    }

    /// Start uploading the bundle folder `bundle` to `server` in the
    /// background (as `rename_to` or its folder name). Returns the transfer id.
    pub fn upload(
        &self,
        server: &str,
        bundle: &str,
        overwrite: bool,
        rename_to: Option<String>,
    ) -> u64 {
        let name = Path::new(bundle)
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| bundle.to_string());
        let tid = self.begin(Direction::Up, server, &name);
        let (table, server, bundle) = (
            self.transfers.clone(),
            server.to_string(),
            bundle.to_string(),
        );
        std::thread::spawn(move || {
            let result = client::upload(
                &server,
                Path::new(&bundle),
                overwrite,
                rename_to.as_deref(),
                &|done, total| set_progress(&table, tid, done, total),
            );
            finish(&table, tid, result.map(|_| None));
        });
        tid
    }

    /// The transfer table, oldest first.
    pub fn transfers(&self) -> Vec<Transfer> {
        self.transfers.lock().map(|t| t.clone()).unwrap_or_default()
    }

    /// Add a running transfer row; returns its id.
    fn begin(&self, direction: Direction, server: &str, name: &str) -> u64 {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        if let Ok(mut table) = self.transfers.lock() {
            while table.len() >= MAX_TRANSFERS {
                match table.iter().position(|t| t.state != TransferState::Running) {
                    Some(i) => {
                        table.remove(i);
                    }
                    None => break,
                }
            }
            table.push(Transfer {
                id,
                direction,
                server: server.to_string(),
                name: name.to_string(),
                bytes_done: 0,
                bytes_total: 0,
                state: TransferState::Running,
            });
        }
        id
    }
}

/// Lists a remote server's songs against a local library; see
/// [`ShareService::lister`].
#[derive(Debug, Clone)]
pub struct Lister {
    roots: Vec<PathBuf>,
}

impl Lister {
    /// The songs `server` shares, each classified against the local library.
    pub fn list(&self, server: &str) -> Result<Vec<ListedSong>, ShareError> {
        let remote = client::list(server)?;
        let local: Vec<(String, Option<String>)> =
            rockcraft_midi::bundle::list_library(&self.roots)
                .into_iter()
                .map(|e| {
                    let hash = archive::chart_hash(&e.dir).ok();
                    (e.name, hash)
                })
                .collect();
        Ok(remote
            .into_iter()
            .map(|song| {
                let local = classify(&song, &local);
                ListedSong { song, local }
            })
            .collect())
    }
}

/// Same chart anywhere → `Same`; else same name → `Different`; else `Absent`.
fn classify(song: &RemoteSong, local: &[(String, Option<String>)]) -> LocalState {
    if local
        .iter()
        .any(|(_, h)| h.as_deref() == Some(song.chart_hash.as_str()))
    {
        LocalState::Same
    } else if local.iter().any(|(n, _)| *n == song.name) {
        LocalState::Different
    } else {
        LocalState::Absent
    }
}

fn with_row(table: &Table, id: u64, f: impl FnOnce(&mut Transfer)) {
    if let Ok(mut t) = table.lock() {
        if let Some(row) = t.iter_mut().find(|r| r.id == id) {
            f(row);
        }
    }
}

fn set_total(table: &Table, id: u64, total: u64) {
    with_row(table, id, |r| r.bytes_total = total);
}

fn set_progress(table: &Table, id: u64, done: u64, total: u64) {
    with_row(table, id, |r| {
        r.bytes_done = done;
        r.bytes_total = total;
    });
}

fn finish(table: &Table, id: u64, result: Result<Option<String>, ShareError>) {
    with_row(table, id, |r| {
        r.state = match result {
            Ok(dir) => TransferState::Done { dir },
            Err(ShareError::Exists(slug)) => TransferState::Exists { slug },
            Err(e) => TransferState::Failed {
                detail: e.to_string(),
            },
        };
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(name: &str, hash: &str) -> RemoteSong {
        RemoteSong {
            id: format!("library~{name}"),
            name: name.into(),
            origin: None,
            note_count: 1,
            duration_us: 1,
            has_backing: false,
            has_video: false,
            size_bytes: 1,
            chart_hash: hash.into(),
        }
    }

    #[test]
    fn classify_same_different_absent() {
        let local = vec![
            ("tune".to_string(), Some("aaa".to_string())),
            ("other".to_string(), Some("bbb".to_string())),
        ];
        assert_eq!(classify(&song("tune", "aaa"), &local), LocalState::Same);
        // A renamed copy with the same chart is still "same".
        assert_eq!(classify(&song("tune-2", "bbb"), &local), LocalState::Same);
        assert_eq!(
            classify(&song("tune", "ccc"), &local),
            LocalState::Different
        );
        assert_eq!(classify(&song("new", "ddd"), &local), LocalState::Absent);
    }
}
