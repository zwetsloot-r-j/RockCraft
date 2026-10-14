//! The LAN share client (M26-B): blocking, plain HTTP, home network only.
//!
//! `server` is always `"host:port"`. Transfers stream through anonymous temp
//! files, never whole archives in memory.

use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use crate::archive;
use crate::error::ShareError;
use crate::server::{RemoteSong, ServerInfo};

/// Copy chunk size, and the granularity of progress callbacks.
const CHUNK: usize = 64 * 1024;

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(3))
        .timeout_read(Duration::from_secs(30))
        .timeout_write(Duration::from_secs(30))
        .build()
}

/// `http://<server>/api/v1<path>`.
fn url(server: &str, path: &str) -> String {
    format!("http://{server}/api/v1{path}")
}

/// Percent-encode a path segment (keep RFC 3986 unreserved characters).
fn encode(segment: &str) -> String {
    segment
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}

/// Map a ureq error: an error status keeps the server's `error` text.
fn call_error(server: &str, e: ureq::Error) -> ShareError {
    match e {
        ureq::Error::Status(code, resp) => {
            let detail = resp
                .into_string()
                .ok()
                .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                .and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string))
                .unwrap_or_default();
            ShareError::Status { code, detail }
        }
        ureq::Error::Transport(t) => ShareError::Unreachable {
            server: server.to_string(),
            detail: t.to_string(),
        },
    }
}

fn get_json<T: serde::de::DeserializeOwned>(server: &str, path: &str) -> Result<T, ShareError> {
    let resp = agent()
        .get(&url(server, path))
        .call()
        .map_err(|e| call_error(server, e))?;
    serde_json::from_reader(resp.into_reader()).map_err(|e| ShareError::BadResponse(e.to_string()))
}

/// `GET /info`; also checks the peer is a RockCraft share server.
pub fn info(server: &str) -> Result<ServerInfo, ShareError> {
    let info: ServerInfo = get_json(server, "/info")?;
    if info.app != "rockcraft" {
        return Err(ShareError::BadResponse(format!(
            "not RockCraft ({})",
            info.app
        )));
    }
    Ok(info)
}

/// `GET /songs`.
pub fn list(server: &str) -> Result<Vec<RemoteSong>, ShareError> {
    get_json(server, "/songs")
}

/// Copy `from` into `to` in chunks, reporting `(done, total)` after each.
fn copy_with_progress(
    mut from: impl Read,
    mut to: impl Write,
    total: u64,
    progress: &dyn Fn(u64, u64),
) -> io::Result<u64> {
    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    progress(0, total);
    loop {
        let n = from.read(&mut buf)?;
        if n == 0 {
            break;
        }
        to.write_all(&buf[..n])?;
        done += n as u64;
        progress(done, total.max(done));
    }
    to.flush()?;
    Ok(done)
}

/// Download `song` (with its video) into `library/<slug>`, where the slug is
/// `rename_to` or the song's name. A taken slug fails with
/// [`ShareError::Exists`] **before** any transfer, unless `overwrite`.
/// `progress(done, total)` follows the bytes received. Returns the bundle dir.
pub fn download(
    server: &str,
    song: &RemoteSong,
    library: &Path,
    overwrite: bool,
    rename_to: Option<&str>,
    progress: &dyn Fn(u64, u64),
) -> Result<PathBuf, ShareError> {
    let slug = rockcraft_midi::bundle::slug(rename_to.unwrap_or(&song.name));
    if slug.is_empty() {
        return Err(ShareError::BadResponse(format!(
            "bad song name {:?}",
            song.name
        )));
    }
    if !overwrite && library.join(&slug).exists() {
        return Err(ShareError::Exists(slug));
    }
    let resp = agent()
        .get(&url(
            server,
            &format!("/songs/{}/archive", encode(&song.id)),
        ))
        .call()
        .map_err(|e| match call_error(server, e) {
            ShareError::Status { code: 404, .. } => ShareError::NotFound(song.id.clone()),
            other => other,
        })?;
    let total = resp
        .header("Content-Length")
        .and_then(|v| v.parse::<u64>().ok())
        .unwrap_or(song.size_bytes);
    let mut file = tempfile::tempfile()?;
    copy_with_progress(resp.into_reader(), &mut file, total, progress)?;
    file.seek(SeekFrom::Start(0))?;
    Ok(archive::import_song_archive(
        io::BufReader::new(file),
        library,
        overwrite,
        Some(&slug),
    )?)
}

/// A reader that reports `(done, total)` as it is read.
struct Progress<'a, R> {
    inner: R,
    done: u64,
    total: u64,
    report: &'a dyn Fn(u64, u64),
}

impl<R: Read> Read for Progress<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        (self.report)(self.done, self.total);
        Ok(n)
    }
}

/// Upload the bundle folder `bundle` to the server as `rename_to` or its
/// folder name (slugged). The server's "name taken" (409) comes back as
/// [`ShareError::Exists`] unless `overwrite`. `progress(done, total)` follows
/// the bytes sent. Returns the slug it was stored under.
pub fn upload(
    server: &str,
    bundle: &Path,
    overwrite: bool,
    rename_to: Option<&str>,
    progress: &dyn Fn(u64, u64),
) -> Result<String, ShareError> {
    let folder = bundle
        .canonicalize()
        .unwrap_or_else(|_| bundle.to_path_buf())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let slug = rockcraft_midi::bundle::slug(rename_to.unwrap_or(&folder));
    if slug.is_empty() {
        return Err(ShareError::BadResponse(format!("bad song name {folder:?}")));
    }
    let mut file = tempfile::tempfile()?;
    archive::pack(bundle, &mut file)?;
    let total = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    progress(0, total);
    let reader = Progress {
        inner: io::BufReader::new(file),
        done: 0,
        total,
        report: progress,
    };
    let path = format!(
        "/songs/{}?overwrite={}",
        encode(&slug),
        if overwrite { 1 } else { 0 }
    );
    agent()
        .put(&url(server, &path))
        .set("Content-Type", "application/zip")
        .set("Content-Length", &total.to_string())
        .send(reader)
        .map_err(|e| match call_error(server, e) {
            ShareError::Status { code: 409, .. } => ShareError::Exists(slug.clone()),
            other => other,
        })?;
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encode_keeps_unreserved_and_escapes_the_rest() {
        assert_eq!(encode("library~frog-song_1.2"), "library~frog-song_1.2");
        assert_eq!(encode("a b/ü"), "a%20b%2F%C3%BC");
    }
}
