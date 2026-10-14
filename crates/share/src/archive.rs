//! The `.rcsong` archive: one song bundle as a single zip file (M26-A).
//!
//! A bundle is a folder holding `meta.json` + `song.mid`, the media files that
//! `meta.json` references (backing, video, background images) and the desktop
//! app's optional sidecars. Folders also collect junk (`song.mid.bak*`,
//! `song.mid.orig`, …) that must not travel, so the archive carries exactly
//! [`bundle_files`] and nothing else.
//!
//! Unpacking is strict — flat file names only, only files the archive's own
//! `meta.json` accounts for, a size cap — and goes through a `.partial` folder
//! that is renamed into place only on success, so a bad archive never leaves a
//! half-written bundle in the library.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Seek, Write};
use std::path::{Path, PathBuf};

use rockcraft_core::RecordingMeta;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// File extension of a song archive (without the dot).
pub const ARCHIVE_EXT: &str = "rcsong";

/// The archive's own descriptor, always its first entry.
const MANIFEST_NAME: &str = "manifest.json";
/// The bundle manifest (`core::RecordingMeta`).
const META_NAME: &str = "meta.json";
/// The chart.
const MIDI_NAME: &str = "song.mid";
/// Optional desktop-app sidecars that travel with a bundle when present.
const SIDECARS: [&str; 2] = ["alignment.json", "transcription.json"];
/// The only archive format version this build reads and writes.
const FORMAT_VERSION: u32 = 1;
/// Cap on the total uncompressed size of an archive being unpacked (4 GiB).
const MAX_UNPACKED_BYTES: u64 = 4 << 30;
/// Extensions of media that is already compressed: stored, not deflated.
const STORED_EXTS: [&str; 12] = [
    "mp4", "webm", "mkv", "mov", "ogg", "mp3", "m4a", "flac", "png", "jpg", "jpeg", "webp",
];

/// The `manifest.json` entry that identifies a song archive.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Always `"rcsong"`.
    pub format: String,
    /// Format version; this build handles [`FORMAT_VERSION`] only.
    pub version: u32,
    /// The bundle folder's name on the machine that packed it.
    pub name: String,
}

/// What [`pack`] wrote.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PackSummary {
    /// Bundle files written (the manifest not included).
    pub files: usize,
    /// Their total uncompressed size in bytes.
    pub bytes: u64,
}

/// A failed archive operation.
#[derive(Debug)]
pub enum ArchiveError {
    /// Disk or stream I/O failed.
    Io(io::Error),
    /// The zip container is malformed.
    Zip(String),
    /// A required or referenced file is missing.
    Missing(String),
    /// A file name isn't a plain, flat name (path separators, `..`, drive…).
    BadName(String),
    /// The archive holds a file the bundle doesn't account for.
    Unexpected(String),
    /// `manifest.json` is missing, malformed, or of an unknown version.
    Manifest(String),
    /// `meta.json` doesn't parse.
    Meta(String),
    /// The archive unpacks to more than the size cap.
    TooLarge,
    /// The destination folder already exists (a raw [`unpack`] target).
    DestExists(PathBuf),
    /// A library bundle with this slug already exists. Displays as
    /// `exists:<slug>` — the convention frontends key their
    /// Replace / Keep both prompt on.
    Exists(String),
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ArchiveError::Io(e) => write!(f, "i/o error: {e}"),
            ArchiveError::Zip(e) => write!(f, "not a valid song archive: {e}"),
            ArchiveError::Missing(n) => write!(f, "missing file: {n}"),
            ArchiveError::BadName(n) => write!(f, "invalid file name in archive: {n:?}"),
            ArchiveError::Unexpected(n) => write!(f, "unexpected file in archive: {n}"),
            ArchiveError::Manifest(e) => write!(f, "bad archive manifest: {e}"),
            ArchiveError::Meta(e) => write!(f, "bad meta.json: {e}"),
            ArchiveError::TooLarge => write!(f, "archive is larger than the 4 GiB limit"),
            ArchiveError::DestExists(p) => write!(f, "destination exists: {}", p.display()),
            ArchiveError::Exists(slug) => write!(f, "exists:{slug}"),
        }
    }
}

impl std::error::Error for ArchiveError {}

impl From<io::Error> for ArchiveError {
    fn from(e: io::Error) -> Self {
        ArchiveError::Io(e)
    }
}

impl From<zip::result::ZipError> for ArchiveError {
    fn from(e: zip::result::ZipError) -> Self {
        match e {
            zip::result::ZipError::Io(e) => ArchiveError::Io(e),
            other => ArchiveError::Zip(other.to_string()),
        }
    }
}

/// Whether `name` is a plain file name: non-empty, no path separators, no
/// `.`/`..`, no drive prefix or other colon.
fn is_flat_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', ':', '\0'])
}

/// The media files `meta` references, in archive order (backing, video,
/// backgrounds), de-duplicated. Each must be a flat name.
fn referenced_files(meta: &RecordingMeta) -> Result<Vec<String>, ArchiveError> {
    let mut out: Vec<String> = Vec::new();
    let refs = meta
        .backing
        .iter()
        .map(|b| b.file.as_str())
        .chain(meta.video.iter().map(|v| v.file.as_str()))
        .chain(meta.backgrounds.iter().map(|b| b.file.as_str()));
    for name in refs {
        if !is_flat_name(name) {
            return Err(ArchiveError::BadName(name.to_string()));
        }
        if name != META_NAME && name != MIDI_NAME && !out.iter().any(|n| n == name) {
            out.push(name.to_string());
        }
    }
    Ok(out)
}

fn parse_meta(json: &str) -> Result<RecordingMeta, ArchiveError> {
    RecordingMeta::from_json(json).map_err(|e| ArchiveError::Meta(e.to_string()))
}

/// The files that make up the bundle in `dir`, in archive order: `meta.json`,
/// `song.mid`, the referenced media, then the sidecars that are present.
/// Errors if `meta.json` or `song.mid` is missing, or a referenced file is.
pub fn bundle_files(dir: &Path) -> Result<Vec<String>, ArchiveError> {
    for required in [META_NAME, MIDI_NAME] {
        if !dir.join(required).is_file() {
            return Err(ArchiveError::Missing(required.to_string()));
        }
    }
    let meta = parse_meta(&fs::read_to_string(dir.join(META_NAME))?)?;
    let mut files = vec![META_NAME.to_string(), MIDI_NAME.to_string()];
    for name in referenced_files(&meta)? {
        if !dir.join(&name).is_file() {
            return Err(ArchiveError::Missing(name));
        }
        files.push(name);
    }
    for sidecar in SIDECARS {
        if dir.join(sidecar).is_file() && !files.iter().any(|f| f == sidecar) {
            files.push(sidecar.to_string());
        }
    }
    Ok(files)
}

/// Compression for an archive entry: already-compressed media is stored.
fn method_for(name: &str) -> CompressionMethod {
    let ext = Path::new(name)
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    if STORED_EXTS.contains(&ext.as_str()) {
        CompressionMethod::Stored
    } else {
        CompressionMethod::Deflated
    }
}

/// Write the bundle in `dir` as a `.rcsong` archive to `out`: the manifest
/// first, then [`bundle_files`]. Media that is already compressed is stored;
/// everything else (wav, mid, json) is deflated.
pub fn pack(dir: &Path, out: impl Write + Seek) -> Result<PackSummary, ArchiveError> {
    let files = bundle_files(dir)?;
    let name = dir
        .canonicalize()
        .unwrap_or_else(|_| dir.to_path_buf())
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "song".to_string());
    let manifest = Manifest {
        format: ARCHIVE_EXT.to_string(),
        version: FORMAT_VERSION,
        name,
    };
    let mut zip = ZipWriter::new(out);
    zip.start_file(
        MANIFEST_NAME,
        SimpleFileOptions::default().compression_method(CompressionMethod::Deflated),
    )?;
    zip.write_all(&serde_json::to_vec_pretty(&manifest).expect("manifest serialises"))?;
    let mut bytes = 0u64;
    for file in &files {
        let mut src = File::open(dir.join(file))?;
        let len = src.metadata()?.len();
        let options = SimpleFileOptions::default()
            .compression_method(method_for(file))
            .large_file(len >= u32::MAX as u64);
        zip.start_file(file.as_str(), options)?;
        bytes += io::copy(&mut src, &mut zip)?;
    }
    zip.finish()?;
    Ok(PackSummary {
        files: files.len(),
        bytes,
    })
}

/// Read and check an archive's `manifest.json` without unpacking it.
pub fn read_manifest(archive: impl Read + Seek) -> Result<Manifest, ArchiveError> {
    let mut zip = ZipArchive::new(archive)?;
    manifest_of(&mut zip)
}

fn manifest_of<R: Read + Seek>(zip: &mut ZipArchive<R>) -> Result<Manifest, ArchiveError> {
    let mut text = String::new();
    zip.by_name(MANIFEST_NAME)
        .map_err(|_| ArchiveError::Manifest("no manifest.json".into()))?
        .take(64 * 1024)
        .read_to_string(&mut text)?;
    let manifest: Manifest =
        serde_json::from_str(&text).map_err(|e| ArchiveError::Manifest(e.to_string()))?;
    if manifest.format != ARCHIVE_EXT {
        return Err(ArchiveError::Manifest(format!(
            "format {:?} is not {ARCHIVE_EXT:?}",
            manifest.format
        )));
    }
    if manifest.version != FORMAT_VERSION {
        return Err(ArchiveError::Manifest(format!(
            "version {} is not supported (this build reads {FORMAT_VERSION})",
            manifest.version
        )));
    }
    Ok(manifest)
}

/// Unpack a `.rcsong` into the new folder `dest`, which must not exist.
///
/// Strict: every entry must be a flat file name and one of `meta.json`,
/// `song.mid`, a known sidecar, or a file the archive's own `meta.json`
/// references; `meta.json`, `song.mid` and every referenced file must be
/// present; the total unpacked size is capped at 4 GiB. Writes into
/// `dest.partial` and renames it into place only on success.
pub fn unpack(archive: impl Read + Seek, dest: &Path) -> Result<(), ArchiveError> {
    if dest.exists() {
        return Err(ArchiveError::DestExists(dest.to_path_buf()));
    }
    let mut zip = ZipArchive::new(archive)?;
    manifest_of(&mut zip)?;

    let mut meta_json = String::new();
    zip.by_name(META_NAME)
        .map_err(|_| ArchiveError::Missing(META_NAME.into()))?
        .take(16 << 20)
        .read_to_string(&mut meta_json)?;
    let referenced = referenced_files(&parse_meta(&meta_json)?)?;
    let allowed: BTreeSet<&str> = [META_NAME, MIDI_NAME]
        .into_iter()
        .chain(SIDECARS)
        .chain(referenced.iter().map(String::as_str))
        .collect();

    // Validate every entry before writing anything.
    let mut present: BTreeSet<String> = BTreeSet::new();
    let mut declared = 0u64;
    for i in 0..zip.len() {
        let entry = zip.by_index_raw(i)?;
        let name = entry.name().to_string();
        if name == MANIFEST_NAME {
            continue;
        }
        if !is_flat_name(&name) {
            return Err(ArchiveError::BadName(name));
        }
        if !allowed.contains(name.as_str()) {
            return Err(ArchiveError::Unexpected(name));
        }
        declared = declared.saturating_add(entry.size());
        present.insert(name);
    }
    if declared > MAX_UNPACKED_BYTES {
        return Err(ArchiveError::TooLarge);
    }
    for required in [META_NAME, MIDI_NAME]
        .into_iter()
        .chain(referenced.iter().map(String::as_str))
    {
        if !present.contains(required) {
            return Err(ArchiveError::Missing(required.to_string()));
        }
    }

    let partial = partial_path(dest);
    if partial.exists() {
        fs::remove_dir_all(&partial)?;
    }
    fs::create_dir_all(&partial)?;
    let result = extract_into(&mut zip, &partial, &present);
    match result {
        Ok(()) => {
            fs::rename(&partial, dest)?;
            Ok(())
        }
        Err(e) => {
            let _ = fs::remove_dir_all(&partial);
            Err(e)
        }
    }
}

/// `<dest>.partial`, next to `dest`.
fn partial_path(dest: &Path) -> PathBuf {
    let mut name = dest.file_name().unwrap_or_default().to_os_string();
    name.push(".partial");
    dest.with_file_name(name)
}

/// Copy the validated entries `names` into `dir`, enforcing the size cap on
/// the actual bytes (headers can lie).
fn extract_into<R: Read + Seek>(
    zip: &mut ZipArchive<R>,
    dir: &Path,
    names: &BTreeSet<String>,
) -> Result<(), ArchiveError> {
    let mut budget = MAX_UNPACKED_BYTES;
    for name in names {
        let entry = zip.by_name(name)?;
        let mut out = File::create(dir.join(name))?;
        let copied = io::copy(&mut entry.take(budget + 1), &mut out)?;
        if copied > budget {
            return Err(ArchiveError::TooLarge);
        }
        budget -= copied;
    }
    Ok(())
}

/// Fingerprint of the chart in `dir`: SHA-256 over the `meta.json` bytes,
/// then the `song.mid` bytes, as lowercase hex. Lets a peer tell "same song"
/// from "same name, other edit" without moving any media.
pub fn chart_hash(dir: &Path) -> Result<String, ArchiveError> {
    let mut hasher = Sha256::new();
    hasher.update(fs::read(dir.join(META_NAME))?);
    hasher.update(fs::read(dir.join(MIDI_NAME))?);
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

/// Export the bundle `bundle` to the archive file `path`, appending
/// `.rcsong` when `path` has no extension. Returns the path written. Writes a
/// temp file next to it first, so a failed export never leaves a truncated
/// archive under the real name.
pub fn export_song_file(bundle: &Path, path: &Path) -> Result<PathBuf, ArchiveError> {
    let path = if path.extension().is_none() {
        path.with_extension(ARCHIVE_EXT)
    } else {
        path.to_path_buf()
    };
    let mut tmp_name = path.file_name().unwrap_or_default().to_os_string();
    tmp_name.push(".tmp");
    let tmp = path.with_file_name(tmp_name);
    let result = File::create(&tmp)
        .map_err(ArchiveError::from)
        .and_then(|f| pack(bundle, io::BufWriter::new(f)));
    match result {
        Ok(_) => {
            fs::rename(&tmp, &path)?;
            Ok(path)
        }
        Err(e) => {
            let _ = fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// Import the archive file `path` into `library/<slug>`, where the slug comes
/// from `name` when given (the frontend's "Keep both" pick) or else from the
/// archive's manifest name. Returns the new bundle folder.
///
/// If the folder exists: without `overwrite` this fails with
/// [`ArchiveError::Exists`] (`exists:<slug>`); with it, the old folder is
/// replaced only after the new one has unpacked successfully.
pub fn import_song_file(
    path: &Path,
    library: &Path,
    overwrite: bool,
    name: Option<&str>,
) -> Result<PathBuf, ArchiveError> {
    let manifest = read_manifest(File::open(path)?)?;
    let slug = rockcraft_midi::bundle::slug(name.unwrap_or(&manifest.name));
    if slug.is_empty() {
        return Err(ArchiveError::BadName(
            name.unwrap_or(&manifest.name).to_string(),
        ));
    }
    let dest = library.join(&slug);
    if dest.exists() && !overwrite {
        return Err(ArchiveError::Exists(slug));
    }
    fs::create_dir_all(library)?;
    let incoming = library.join(format!("{slug}.incoming"));
    if incoming.exists() {
        fs::remove_dir_all(&incoming)?;
    }
    unpack(io::BufReader::new(File::open(path)?), &incoming)?;
    if dest.exists() {
        let old = library.join(format!("{slug}.old"));
        if old.exists() {
            fs::remove_dir_all(&old)?;
        }
        fs::rename(&dest, &old)?;
        if let Err(e) = fs::rename(&incoming, &dest) {
            // Put the original back rather than lose it.
            let _ = fs::rename(&old, &dest);
            return Err(e.into());
        }
        let _ = fs::remove_dir_all(&old);
    } else {
        fs::rename(&incoming, &dest)?;
    }
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// A bundle folder `name` under `root` with a backing wav, a video, an
    /// alignment sidecar, and backup junk. Returns its path.
    fn make_bundle(root: &Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        fs::create_dir_all(&dir).unwrap();
        let meta = r#"{"midi_file":"song.mid",
            "backing":{"file":"backing.wav","audio_start_us":0},
            "video":{"file":"source.mp4","offset_us":0},
            "version":1}"#;
        fs::write(dir.join("meta.json"), meta).unwrap();
        fs::write(dir.join("song.mid"), b"MThd-fake-midi").unwrap();
        fs::write(dir.join("backing.wav"), vec![7u8; 50_000]).unwrap();
        fs::write(dir.join("source.mp4"), vec![3u8; 20_000]).unwrap();
        fs::write(dir.join("alignment.json"), b"{}").unwrap();
        fs::write(dir.join("song.mid.bak2"), b"old").unwrap();
        fs::write(dir.join("song.mid.orig"), b"older").unwrap();
        dir
    }

    fn packed(dir: &Path) -> Vec<u8> {
        let mut buf = Cursor::new(Vec::new());
        pack(dir, &mut buf).unwrap();
        buf.into_inner()
    }

    /// Build a raw zip from `(name, bytes)` entries, for hostile-archive tests.
    fn raw_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
        for (name, bytes) in entries {
            zip.start_file(*name, SimpleFileOptions::default()).unwrap();
            zip.write_all(bytes).unwrap();
        }
        zip.finish().unwrap().into_inner()
    }

    const MANIFEST: &[u8] = br#"{"format":"rcsong","version":1,"name":"x"}"#;
    const META: &[u8] = br#"{"midi_file":"song.mid","version":1}"#;

    #[test]
    fn bundle_files_lists_the_song_and_skips_junk() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_bundle(tmp.path(), "tune");
        assert_eq!(
            bundle_files(&dir).unwrap(),
            vec![
                "meta.json",
                "song.mid",
                "backing.wav",
                "source.mp4",
                "alignment.json"
            ]
        );
    }

    #[test]
    fn bundle_files_errors_on_a_missing_referenced_file() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_bundle(tmp.path(), "tune");
        fs::remove_file(dir.join("backing.wav")).unwrap();
        assert!(matches!(
            bundle_files(&dir),
            Err(ArchiveError::Missing(n)) if n == "backing.wav"
        ));
    }

    #[test]
    fn pack_unpack_round_trips_and_compresses_only_the_wav() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_bundle(tmp.path(), "tune");
        let bytes = packed(&dir);

        let mut zip = ZipArchive::new(Cursor::new(bytes.clone())).unwrap();
        assert_eq!(zip.by_index(0).unwrap().name(), "manifest.json");
        assert_eq!(
            zip.by_name("backing.wav").unwrap().compression(),
            CompressionMethod::Deflated
        );
        assert_eq!(
            zip.by_name("source.mp4").unwrap().compression(),
            CompressionMethod::Stored
        );
        assert!(zip.by_name("song.mid.bak2").is_err());

        let dest = tmp.path().join("copy");
        unpack(Cursor::new(bytes), &dest).unwrap();
        for f in bundle_files(&dir).unwrap() {
            assert_eq!(
                fs::read(dir.join(&f)).unwrap(),
                fs::read(dest.join(&f)).unwrap(),
                "{f} differs"
            );
        }
        assert!(!dest.join("song.mid.orig").exists());
        assert!(!tmp.path().join("copy.partial").exists());
    }

    /// Unpacking `bytes` fails and leaves neither `dest` nor `dest.partial`.
    fn assert_rejected(bytes: Vec<u8>) -> ArchiveError {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("out");
        let err = unpack(Cursor::new(bytes), &dest).unwrap_err();
        assert!(!dest.exists());
        assert!(!tmp.path().join("out.partial").exists());
        err
    }

    #[test]
    fn unpack_rejects_path_tricks() {
        for bad in ["../evil", "a/b.mid", "C:x", "..\\evil"] {
            let err = assert_rejected(raw_zip(&[
                ("manifest.json", MANIFEST),
                ("meta.json", META),
                ("song.mid", b"m"),
                (bad, b"x"),
            ]));
            assert!(matches!(err, ArchiveError::BadName(_)), "{bad}: {err}");
        }
    }

    #[test]
    fn unpack_rejects_unreferenced_files() {
        let err = assert_rejected(raw_zip(&[
            ("manifest.json", MANIFEST),
            ("meta.json", META),
            ("song.mid", b"m"),
            ("payload.exe", b"x"),
        ]));
        assert!(matches!(err, ArchiveError::Unexpected(n) if n == "payload.exe"));
    }

    #[test]
    fn unpack_rejects_missing_or_unknown_manifest() {
        let err = assert_rejected(raw_zip(&[("meta.json", META), ("song.mid", b"m")]));
        assert!(matches!(err, ArchiveError::Manifest(_)));
        let v2 = br#"{"format":"rcsong","version":2,"name":"x"}"#;
        let err = assert_rejected(raw_zip(&[
            ("manifest.json", v2),
            ("meta.json", META),
            ("song.mid", b"m"),
        ]));
        assert!(matches!(err, ArchiveError::Manifest(_)));
    }

    #[test]
    fn unpack_rejects_a_missing_chart_or_referenced_file() {
        let err = assert_rejected(raw_zip(&[("manifest.json", MANIFEST), ("meta.json", META)]));
        assert!(matches!(err, ArchiveError::Missing(n) if n == "song.mid"));
        let meta = br#"{"midi_file":"song.mid","backing":{"file":"b.wav","audio_start_us":0}}"#;
        let err = assert_rejected(raw_zip(&[
            ("manifest.json", MANIFEST),
            ("meta.json", meta),
            ("song.mid", b"m"),
        ]));
        assert!(matches!(err, ArchiveError::Missing(n) if n == "b.wav"));
    }

    #[test]
    fn unpack_refuses_an_existing_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_bundle(tmp.path(), "tune");
        let err = unpack(Cursor::new(packed(&dir)), &dir).unwrap_err();
        assert!(matches!(err, ArchiveError::DestExists(_)));
    }

    #[test]
    fn chart_hash_is_stable_and_tracks_the_chart() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_bundle(tmp.path(), "tune");
        let h1 = chart_hash(&dir).unwrap();
        assert_eq!(h1, chart_hash(&dir).unwrap());
        assert_eq!(h1.len(), 64);
        // Media changes don't matter; the chart does.
        fs::write(dir.join("backing.wav"), b"other audio").unwrap();
        assert_eq!(h1, chart_hash(&dir).unwrap());
        fs::write(dir.join("song.mid"), b"MThd-edited").unwrap();
        assert_ne!(h1, chart_hash(&dir).unwrap());
    }

    #[test]
    fn export_then_import_with_conflicts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = make_bundle(tmp.path(), "My Tune");
        let out = export_song_file(&dir, &tmp.path().join("shared")).unwrap();
        assert_eq!(out, tmp.path().join("shared.rcsong"));
        assert!(!tmp.path().join("shared.rcsong.tmp").exists());

        let library = tmp.path().join("library");
        let first = import_song_file(&out, &library, false, None).unwrap();
        assert_eq!(first, library.join("my-tune"));
        assert!(first.join("song.mid").is_file());

        let err = import_song_file(&out, &library, false, None).unwrap_err();
        assert_eq!(err.to_string(), "exists:my-tune");

        // Overwrite replaces the folder's contents.
        fs::write(first.join("song.mid.bak2"), b"stale").unwrap();
        let again = import_song_file(&out, &library, true, None).unwrap();
        assert_eq!(again, first);
        assert!(!first.join("song.mid.bak2").exists());
        assert!(!library.join("my-tune.old").exists());
        assert!(!library.join("my-tune.incoming").exists());

        // Keep both: an explicit name.
        let both = import_song_file(&out, &library, false, Some("my-tune-2")).unwrap();
        assert_eq!(both, library.join("my-tune-2"));
    }
}
