# M26-A — Song archive format (`.rcsong`) + export/import song file

> Milestone: M26 — LAN song sharing · Issue: #345 · Suggested tier: sonnet
> Branch: `claude/m26-song-archive`

## Goal

Give a song a single-file form, so it can be moved between machines.
`.rcsong` is a zip of the bundle folder. M26-B sends these files over the
home network, and on its own it also gives "Export song to file" / "Import
song from file" (USB stick, cloud folder, mail).

## Context

- A bundle is a folder: `song.mid` + `meta.json` (`core::RecordingMeta`),
  plus the files `meta.json` references:
  - `backing.file`
  - `video.file`
  - each `backgrounds[].file`
- **Optional sidecars** written by the desktop app:
  - `alignment.json` (`tauri-app/src-tauri/src/alignment.rs`)
  - `transcription.json` (`tauri-app/src-tauri/src/transcription.rs`)
- Real folders also collect junk that must **not** travel: `song.mid.bak*`,
  `song.mid.orig`, editor temp files.
- Sizes: a chart is a few KB, `backing.wav` 25–45 MB (uncompressed PCM), and
  the video 10–150 MB (already compressed).
- Library roots and naming: `crates/midi/src/bundle.rs`:
  - `library_root()`, `default_scan_roots()`
  - `slug()`
  - `read_entry` / `list_library` / `LibraryEntry`
- Host-command rules: CLAUDE.md "Agent-control API".

## What to do

### 1. New crate `crates/share` (`rockcraft-share`)

It depends on `rockcraft-core`, `rockcraft-midi` (library helpers), `serde`,
`serde_json`, `zip` (default features off, `deflate` on) and `sha2`. Module
`archive.rs`:

```rust
/// Files that make up a bundle, in archive order: meta.json, song.mid, then
/// referenced media (backing, video, backgrounds), then known sidecars.
/// Errors if song.mid or meta.json is missing, or a referenced file is.
pub fn bundle_files(dir: &Path) -> Result<Vec<String>, ArchiveError>;

/// Write `dir` as a .rcsong to `out` (any `Write + Seek`). Media that is
/// already compressed (mp4, webm, mkv, mov, ogg, mp3, m4a, flac, png, jpg,
/// jpeg, webp) is **stored**; everything else (wav, mid, json) is deflated.
pub fn pack(dir: &Path, out: impl Write + Seek) -> Result<PackSummary, ArchiveError>;

/// Unpack a .rcsong into a new folder `dest` (must not exist). Strict:
/// - only flat file names, no `/`, `\`, `..`, absolute paths, or drive prefixes;
/// - every entry must be meta.json, song.mid, a known sidecar, or a file that
///   the archive's own meta.json references; anything else is an error;
/// - a total uncompressed limit of 4 GiB;
/// - meta.json must parse as `RecordingMeta`.
/// Unpacks into `dest.partial`, then renames, so a failure never leaves a
/// half-written bundle in the library.
pub fn unpack(archive: impl Read + Seek, dest: &Path) -> Result<(), ArchiveError>;

/// Fingerprint of the chart: sha256 over meta.json bytes, then song.mid
/// bytes, hex. Lets a peer tell "same song" from "same name, other edit".
pub fn chart_hash(dir: &Path) -> Result<String, ArchiveError>;
```

`ArchiveError` is a plain enum with `Display`. Add a `manifest.json` entry
first in the zip: `{ "format": "rcsong", "version": 1, "name": "<folder
name>" }`. `unpack` checks it and rejects unknown major versions.

### 2. Host commands

- `ExportSongFile { bundle: String, path: String }` packs bundle dir `bundle`
  to `path`, appending `.rcsong` when there is no extension. It returns the
  written path.
- `ImportSongFile { path: String, overwrite: bool }` unpacks into
  `library_root()/<slug(manifest name)>`:
  - if that folder exists and `overwrite` is false: error `exists:<slug>`;
  - with `overwrite`, the old folder is replaced only after the new one has
    unpacked successfully.

  It returns the new bundle dir.
- Both frontends implement them (TUI included: it's pure disk I/O). Add the
  names-list, catalog and parity-test entries.
- Tauri: `library_export_song` / `library_import_song` IPC commands paired
  with these.

### 3. Desktop library screen

- `x`: export the selected song through a save dialog (`<name>.rcsong`), then
  flash the result.
- `i`: import through an open dialog filtered to `.rcsong`. On `exists:` ask
  "Replace / Keep both / Cancel": Keep both imports as `<slug>-2` (`-3`, …).
  Afterwards rescan the list and select the imported song.
- Add both keys to the screen's key legend.

## Tests

- `bundle_files` lists meta, mid and referenced media plus present sidecars,
  and skips `song.mid.bak2` and `song.mid.orig`. It errors when a referenced
  backing file is missing.
- A pack → unpack round trip is byte-identical per file. A `.wav` entry is
  deflated and an `.mp4` entry stored (check the zip entry's
  compression method).
- `unpack` rejects:
  - `../evil`, `a/b.mid`, `C:x`;
  - an unreferenced extra file;
  - a missing manifest or a version-2 manifest;
  - an existing `dest`.

  The rejections leave no `dest` or `dest.partial` behind.
- `chart_hash` is stable across two packs and changes when `song.mid` changes.
- The host commands' parity tests and a Tauri-crate test: export, then
  import with `overwrite: false` into a temp library gives `exists:`, and
  with `true` succeeds.

## Scope boundaries (do NOT)

- No networking (M26-B).
- Don't change the bundle layout or `RecordingMeta`.
- No compression of video, and no transcoding.

## Acceptance

- [ ] `cargo fmt --all --check`, `cargo clippy --workspace --all-targets`, `cargo test --workspace` green
- [ ] Tauri crate tests + frontend `tsc`/vitest/build green locally
- [ ] PR against `main` from the branch above, `Closes #345`
