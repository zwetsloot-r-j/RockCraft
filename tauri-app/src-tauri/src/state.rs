//! Managed application state and the command bodies that drive it.
//!
//! The Tauri backend owns a single [`rockcraft_core::Composer`] behind a
//! `Mutex` ([`AppState`]) — the live editor brain. The webview drives it
//! through invoke commands (see [`crate::run`]) that mirror the WebSocket
//! control protocol's vocabulary (`run_action` / `query state` / `query help`)
//! without depending on the `rockcraft-control` crate.
//!
//! The command *bodies* live here as free functions taking `&AppState` so they
//! are unit-testable with no Tauri window: the `#[tauri::command]` wrappers in
//! `lib.rs` are thin shims over these.

use std::sync::Mutex;

use rockcraft_core::{
    action_from_name, action_help, slice_segment, ActionError, BackgroundImage, BackgroundStack,
    BackgroundVideo, BackingTrack, Composer, ComposerSnapshot, Effect, Grid, Key, Keyframe,
    NoteView, RecordingMeta, Scale, Segment, Timeline, TrackOrigin, Transform,
};
use rockcraft_midi::{events_to_smf_bytes, smf_bytes_to_events};
use serde::{Deserialize, Serialize};

/// Where to write a saved bundle.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SaveDest {
    /// Quick save — writes to `recordings/take-<timestamp>/`.
    QuickSave,
    /// Named save — writes to `<library_root>/<slug>/`.
    Library { name: String },
    /// Overwrite the bundle the piece was loaded from / last saved to
    /// ([`AppState::current_dir`]) without re-typing a name. Falls back to a
    /// quick-save take when the piece has never been loaded or saved.
    InPlace,
}

/// One kept part for [`split_bundle`] (M10-B): the half-open song-time range
/// `[start_us, end_us)` becomes a new library bundle named `name`.
///
/// The state-side mirror of `rockcraft_control::SegmentSpec`, kept here so this
/// module stays free of the control crate (like [`SaveDest`]); `control.rs`
/// maps between the two.
///
/// Derives `Serialize`/`Deserialize` so the webview can pass kept parts straight
/// to the `split_bundle` invoke command (the same shape the M10-C split editor
/// gathers), mirroring how [`SaveDest`] crosses the IPC boundary.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SplitSegment {
    pub start_us: u64,
    pub end_us: u64,
    pub name: String,
}

/// Tauri-managed state: the live composer behind a `Mutex`, plus the extra
/// frontend-only fields that core deliberately omits.
///
/// One global editor instance for the app. The tick thread and every command
/// lock it, mutate, clone the payloads they need, and release before doing any
/// I/O (event emission), keeping the critical section tight.
pub struct AppState {
    pub composer: Mutex<Composer>,
    /// Current key; mirrored here so `save_bundle` can write `meta.json`
    /// without adding a `Composer::key()` accessor to core.
    pub key: Mutex<Key>,
    /// Track provenance of the loaded/composed piece for `meta.json`.
    pub origin: Mutex<TrackOrigin>,
    /// Whether the timeline has unsaved changes.
    pub dirty: Mutex<bool>,
    /// Path to an attached backing file (bundle-relative when loaded from one).
    pub backing_path: Mutex<Option<std::path::PathBuf>>,
    /// Absolute path to an attached background video, plus its alignment offset
    /// (`videoTime = songTime + offset_us`). `None` when the piece has no
    /// backdrop. Persisted into `meta.json` on save (M9-G); the file is copied
    /// into the bundle next to `song.mid`.
    pub video: Mutex<Option<AttachedVideo>>,
    /// Absolute source image per background layer id (M14-D). The layout and
    /// keyframes live on the composer's `BackgroundStack` — pure state, edited
    /// by `core::Action`s — so this holds only what `core` may not: where the
    /// file is. Exactly the split [`AppState::backing_path`] uses.
    pub background_srcs: Mutex<Vec<AttachedBackground>>,
    /// Directory the piece was last loaded from or saved to. Backs the no-prompt
    /// "save in place" (`SaveDest::InPlace`) — `s` overwrites this bundle without
    /// re-typing a name. `None` for a brand-new composition (falls back to a
    /// quick-save take).
    pub current_dir: Mutex<Option<std::path::PathBuf>>,
}

/// A background image attached to the live editor: the layer id it belongs to
/// and the absolute source file, mirrored here so `save_bundle` can copy it into
/// the bundle. Sibling of [`AttachedVideo`].
#[derive(Debug, Clone)]
pub struct AttachedBackground {
    /// Matches `BackgroundImage::id` on the composer's stack.
    pub id: String,
    /// Absolute source path (or the resolved in-bundle path after a load).
    pub path: std::path::PathBuf,
}

/// A background video attached to the live editor, mirrored frontend-side so
/// `save_bundle` can copy it into the bundle and write `RecordingMeta.video`.
///
/// `path` is the absolute source path the webview picked (or the resolved
/// in-bundle path after a load). `offset_us` mirrors
/// [`rockcraft_core::BackgroundVideo::offset_us`].
#[derive(Debug, Clone)]
pub struct AttachedVideo {
    pub path: std::path::PathBuf,
    pub offset_us: i64,
}

impl AppState {
    /// A fresh, empty composer.
    pub fn new() -> Self {
        let default_key = Key {
            root_pc: 0,
            scale: Scale::Major,
        };
        Self {
            composer: Mutex::new(Composer::new()),
            key: Mutex::new(default_key),
            origin: Mutex::new(TrackOrigin::Composed),
            dirty: Mutex::new(false),
            backing_path: Mutex::new(None),
            video: Mutex::new(None),
            background_srcs: Mutex::new(Vec::new()),
            current_dir: Mutex::new(None),
        }
    }
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

/// The result of a successful [`run_action`]: the effects the action produced
/// (for the frontend to sound, later) plus the resulting state snapshot.
///
/// Mirrors the control protocol's `Response::Ok { effects, state }` shape, but
/// always carries a snapshot — a Tauri command's caller always wants the latest
/// state back.
///
/// `dirty` mirrors [`AppState::dirty`]: true when the timeline has unsaved
/// changes. Included here so every action reply keeps the UI indicator in sync
/// without a separate poll.
#[derive(Debug, Clone, Serialize)]
pub struct ActionReply {
    pub effects: Vec<Effect>,
    pub snapshot: ComposerSnapshot,
    pub dirty: bool,
}

/// Apply a named action to the composer and return the effects + new snapshot.
///
/// `params` is the JSON object of named fields (or `null`/empty for nullary
/// actions). Mirrors `run_action` over the WebSocket control protocol:
/// `action_from_name` → `Composer::apply`. An [`ActionError`] (unknown name or
/// bad params) is flattened to its `Display` string so the command layer can
/// surface it as a plain `Err(String)` to the webview.
///
/// Any action that modifies the timeline sets the `dirty` flag in [`AppState`].
/// The flag is detected via a note-count+content fingerprint: the same cheap
/// approach the TUI's `EditScreen::dispatch` uses.
pub fn run_action(
    state: &AppState,
    name: &str,
    params: &serde_json::Value,
) -> Result<ActionReply, String> {
    let action = action_from_name(name, params).map_err(|e: ActionError| e.to_string())?;
    let mut composer = state.composer.lock().expect("composer mutex poisoned");
    let fp_before = timeline_fingerprint(composer.timeline());
    let effects = composer
        .apply(action)
        .map_err(|e: ActionError| e.to_string())?;
    let snapshot = composer.snapshot();
    drop(composer);
    // Mark dirty if the timeline content changed.
    if timeline_fingerprint_snapshot(&snapshot.notes) != fp_before {
        let mut dirty = state.dirty.lock().expect("dirty mutex poisoned");
        *dirty = true;
    }
    let dirty = *state.dirty.lock().expect("dirty mutex poisoned");
    Ok(ActionReply {
        effects,
        snapshot,
        dirty,
    })
}

/// Cheap timeline fingerprint: a sum of (id, pitch, start_us, dur_us, vel) for
/// all notes. A content change (add/remove/modify) will change the result.
fn timeline_fingerprint(timeline: &Timeline) -> u64 {
    timeline
        .notes()
        .map(|(id, n)| {
            (id.value() as u64)
                .wrapping_add(n.pitch.value() as u64)
                .wrapping_add(n.start_us)
                .wrapping_add(n.dur_us)
                .wrapping_add(n.velocity.value() as u64)
        })
        .fold(0u64, u64::wrapping_add)
}

/// Same fingerprint but over the snapshot's [`NoteView`] slice (used after
/// releasing the composer lock).
fn timeline_fingerprint_snapshot(notes: &[NoteView]) -> u64 {
    notes
        .iter()
        .map(|n| {
            (n.id as u64)
                .wrapping_add(n.pitch as u64)
                .wrapping_add(n.start_us)
                .wrapping_add(n.dur_us)
                .wrapping_add(n.velocity as u64)
        })
        .fold(0u64, u64::wrapping_add)
}

/// Save the current composer timeline to a bundle.
///
/// For [`SaveDest::QuickSave`] the bundle lands in `recordings/take-<stamp>/`.
/// For [`SaveDest::Library`] it goes into `<library_root>/<slug>/` (the
/// `library/` directory next to the binary, or `$ROCKCRAFT_LIBRARY_DIR`).
/// Clears the dirty flag on success; returns the bundle directory as a string.
///
/// Mirrors `EditScreen::save` / `save_to_library` from `crates/tui/src/edit.rs`.
pub fn save_bundle(state: &AppState, dest: SaveDest) -> Result<String, String> {
    let quick_save_dir = || {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        std::path::PathBuf::from("recordings").join(format!("take-{stamp}"))
    };
    let bundle_dir = match &dest {
        SaveDest::QuickSave => quick_save_dir(),
        SaveDest::Library { name } => {
            let slug = rockcraft_midi::bundle::slug(name);
            if slug.is_empty() {
                return Err("empty name — cannot save".to_string());
            }
            rockcraft_midi::bundle::library_root().join(slug)
        }
        // Overwrite the loaded/last-saved bundle; new pieces fall back to a take.
        SaveDest::InPlace => state
            .current_dir
            .lock()
            .expect("current_dir mutex poisoned")
            .clone()
            .unwrap_or_else(quick_save_dir),
    };
    save_bundle_into(state, &bundle_dir)?;
    Ok(bundle_dir.to_string_lossy().into_owned())
}

/// [`save_bundle`] with an explicit destination directory, so tests can target a
/// temp directory without touching `$ROCKCRAFT_LIBRARY_DIR`. Gathers every piece
/// of live state the bundle needs, writes it, and clears the dirty flag.
fn save_bundle_into(state: &AppState, bundle_dir: &std::path::Path) -> Result<(), String> {
    let composer = state.composer.lock().expect("composer mutex poisoned");
    let timeline = composer.timeline().clone();
    let grid = composer.grid();
    let backing_offset_us = composer.backing_offset_us();
    let backgrounds = composer.backgrounds().layers().to_vec();
    drop(composer);

    let key = *state.key.lock().expect("key mutex poisoned");
    let origin = *state.origin.lock().expect("origin mutex poisoned");
    let background_srcs = state
        .background_srcs
        .lock()
        .expect("background_srcs mutex poisoned")
        .clone();
    let backing_path = state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned")
        .clone();
    let video = state.video.lock().expect("video mutex poisoned").clone();

    write_bundle(
        bundle_dir,
        &timeline,
        grid,
        key,
        origin,
        backing_path.as_deref(),
        backing_offset_us,
        video.as_ref(),
        &backgrounds,
        &background_srcs,
    )
    .map_err(|e| e.to_string())?;

    // Clear dirty flag after a successful save.
    *state.dirty.lock().expect("dirty mutex poisoned") = false;
    // Remember where we saved so a subsequent in-place save overwrites the same
    // bundle without a name prompt.
    *state
        .current_dir
        .lock()
        .expect("current_dir mutex poisoned") = Some(bundle_dir.to_path_buf());
    Ok(())
}

/// Whether two paths name the same file on disk.
///
/// A plain `a == b` is not enough: a bundle's media is tracked by *absolute*
/// path while the bundle dir itself is often *relative* (`library/<slug>`, from
/// `library_root()`), so the same file compares unequal and gets copied onto
/// itself — which Windows rejects with a sharing violation. Resolving both
/// sides catches that. Falls back to a textual compare when either side cannot
/// be resolved (e.g. the destination does not exist yet, which already means
/// they are different files).
fn is_same_file(a: &std::path::Path, b: &std::path::Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// Write `song.mid` + `meta.json` (+ optional backing/video copies) into
/// `bundle_dir`.
#[allow(clippy::too_many_arguments)]
fn write_bundle(
    bundle_dir: &std::path::Path,
    timeline: &Timeline,
    grid: Grid,
    key: Key,
    origin: TrackOrigin,
    backing_path: Option<&std::path::Path>,
    backing_offset_us: u64,
    video: Option<&AttachedVideo>,
    backgrounds: &[BackgroundImage],
    background_srcs: &[AttachedBackground],
) -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_dir)?;
    let bytes = events_to_smf_bytes(&timeline.to_events());
    std::fs::write(bundle_dir.join("song.mid"), bytes)?;

    let backing = if let Some(bpath) = backing_path {
        let filename = crate::record_bundle_backing_filename(bpath);
        let dest = bundle_dir.join(&filename);
        // Saving a bundle back over itself — the common case for an imported
        // piece, whose backing already lives in the bundle dir — makes source
        // and destination the same file. Windows fails that copy outright (a
        // sharing violation), which aborted the save *after* song.mid was
        // written but before meta.json, leaving the bundle half-updated and the
        // dirty flag set.
        if !is_same_file(bpath, &dest) {
            std::fs::copy(bpath, &dest)?;
        }
        Some(BackingTrack {
            file: filename,
            audio_start_us: backing_offset_us,
        })
    } else {
        None
    };

    let video = video
        .map(|v| video_meta_for_bundle(bundle_dir, v))
        .transpose()?;

    // Copy each background image in under the bundle-relative name its layer
    // already carries; a layer whose source is missing keeps its keyframes so a
    // re-save never silently drops the animation.
    for layer in backgrounds {
        let Some(src) = background_srcs.iter().find(|b| b.id == layer.id) else {
            continue;
        };
        let dest = bundle_dir.join(&layer.file);
        if !is_same_file(&src.path, &dest) {
            std::fs::copy(&src.path, &dest)?;
        }
    }

    let meta = RecordingMeta {
        midi_file: "song.mid".into(),
        backing,
        grid: Some(grid),
        key: Some(key),
        origin: Some(origin),
        video,
        backgrounds: backgrounds.to_vec(),
        version: 1,
    };
    std::fs::write(bundle_dir.join("meta.json"), meta.to_json())?;
    Ok(())
}

/// Slice the loaded piece into the given kept parts, writing each as its own
/// standalone library bundle (M10-B).
///
/// Each [`SplitSegment`] becomes `<library_root>/<slug>/` containing the subset
/// MIDI (notes shifted to t=0), a **copied** backing/video file when the piece
/// has media (offsets shifted by the segment start), and a `meta.json` carrying
/// the derived offsets, the source piece's `grid`/`key`, and
/// `origin = Edited`. Discarded parts are simply omitted from `segments`
/// (= trimming). The **source** piece, its bundle, and the dirty flag are left
/// untouched — splitting is non-destructive.
///
/// Returns the created bundle directory paths as strings.
pub fn split_bundle(state: &AppState, segments: Vec<SplitSegment>) -> Result<Vec<String>, String> {
    split_bundle_into(state, &rockcraft_midi::bundle::library_root(), segments)
}

/// [`split_bundle`] with an explicit library root, so tests can target a temp
/// directory without touching `$ROCKCRAFT_LIBRARY_DIR`.
fn split_bundle_into(
    state: &AppState,
    library_root: &std::path::Path,
    segments: Vec<SplitSegment>,
) -> Result<Vec<String>, String> {
    if segments.is_empty() {
        return Err("no segments to write".to_string());
    }

    let composer = state.composer.lock().expect("composer mutex poisoned");
    let timeline = composer.timeline().clone();
    let grid = composer.grid();
    let backing_offset_us = composer.backing_offset_us();
    let backgrounds = composer.backgrounds().layers().to_vec();
    drop(composer);

    let key = *state.key.lock().expect("key mutex poisoned");
    let backing_path = state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned")
        .clone();
    let video = state.video.lock().expect("video mutex poisoned").clone();
    let background_srcs: Vec<(String, std::path::PathBuf)> = state
        .background_srcs
        .lock()
        .expect("background_srcs mutex poisoned")
        .iter()
        .map(|b| (b.id.clone(), b.path.clone()))
        .collect();

    // The loaded media references the slicer shifts per segment. The file names
    // match what `write_bundle` would write, so the copied files line up.
    let backing_meta = backing_path.as_ref().map(|p| BackingTrack {
        file: crate::record_bundle_backing_filename(p),
        audio_start_us: backing_offset_us,
    });
    let video_meta = video.as_ref().map(|v| BackgroundVideo {
        file: video_bundle_filename(&v.path),
        offset_us: v.offset_us,
    });

    let mut dirs = Vec::with_capacity(segments.len());
    for seg in segments {
        let slug = rockcraft_midi::bundle::slug(&seg.name);
        if slug.is_empty() {
            return Err(format!(
                "empty name for segment `{}` — cannot save",
                seg.name
            ));
        }
        let sliced = slice_segment(
            &timeline,
            Segment {
                start_us: seg.start_us,
                end_us: seg.end_us,
            },
            backing_meta.as_ref(),
            video_meta.as_ref(),
            &backgrounds,
        );
        let dir = library_root.join(&slug);
        rockcraft_import::write_part_bundle(
            &dir,
            &sliced,
            grid,
            key,
            backing_path.as_deref(),
            video.as_ref().map(|v| v.path.as_path()),
            &background_srcs,
        )
        .map_err(|e| e.to_string())?;
        dirs.push(dir.to_string_lossy().into_owned());
    }
    Ok(dirs)
}

/// Bundle-relative filename for a retained background video, derived from the
/// source extension (defaulting to `source.mp4`).
fn video_bundle_filename(src: &std::path::Path) -> String {
    match src
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
    {
        Some(ext) => format!("background.{ext}"),
        None => "background.mp4".to_string(),
    }
}

/// Copy the attached video into the bundle (idempotent: if `src` already lives
/// inside `bundle_dir`, no copy is made) and return the [`BackgroundVideo`]
/// reference to persist in `meta.json`.
fn video_meta_for_bundle(
    bundle_dir: &std::path::Path,
    v: &AttachedVideo,
) -> std::io::Result<BackgroundVideo> {
    // Resolve rather than compare textually: the attached path is absolute while
    // `bundle_dir` is usually relative, so a plain parent comparison misses the
    // already-in-bundle case and copies the file onto itself.
    let already_in_bundle = v
        .path
        .parent()
        .map(|p| is_same_file(p, bundle_dir))
        .unwrap_or(false);
    let filename = if already_in_bundle {
        v.path
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| video_bundle_filename(&v.path))
    } else {
        let name = video_bundle_filename(&v.path);
        std::fs::copy(&v.path, bundle_dir.join(&name))?;
        name
    };
    Ok(BackgroundVideo {
        file: filename,
        offset_us: v.offset_us,
    })
}

/// Load a bundle directory into the composer, replacing its current timeline.
///
/// Reads `song.mid` (required) and `meta.json` (optional; fallback to defaults
/// when absent). Mirrors `app.rs::open_edit_from_midi`.  Returns the new
/// composer snapshot so the frontend can refresh immediately.
pub fn load_bundle(state: &AppState, dir: &str) -> Result<ComposerSnapshot, String> {
    let bundle_dir = std::path::PathBuf::from(dir);

    // Load the MIDI file (required).
    let midi_path = bundle_dir.join("song.mid");
    let bytes = std::fs::read(&midi_path).map_err(|e| format!("read song.mid: {e}"))?;
    let events = smf_bytes_to_events(&bytes).map_err(|e| format!("parse song.mid: {e}"))?;
    let timeline = Timeline::from_events(&events);

    // Load the meta (optional — legacy bundles have no meta.json).
    type MetaTuple = (
        Grid,
        Key,
        TrackOrigin,
        Option<std::path::PathBuf>,
        u64,
        Option<AttachedVideo>,
        Vec<BackgroundImage>,
    );
    let default_meta = || -> MetaTuple {
        (
            Grid::default_120(),
            Key {
                root_pc: 0,
                scale: Scale::Major,
            },
            TrackOrigin::Edited,
            None,
            0,
            None,
            Vec::new(),
        )
    };
    let (grid, key, origin, backing_path, backing_offset_us, video, backgrounds) =
        match std::fs::read_to_string(bundle_dir.join("meta.json")) {
            Ok(json) => match RecordingMeta::from_json(&json) {
                Ok(meta) => {
                    let grid = meta.grid.unwrap_or_else(Grid::default_120);
                    let key = meta.key.unwrap_or(Key {
                        root_pc: 0,
                        scale: Scale::Major,
                    });
                    let origin = meta.origin.unwrap_or(TrackOrigin::Edited);
                    let (bpath, boffset) = meta
                        .backing
                        .map(|b| {
                            // Resolve the bundle-relative filename to an absolute path.
                            let abs = bundle_dir.join(&b.file);
                            (Some(abs), b.audio_start_us)
                        })
                        .unwrap_or((None, 0));
                    // Resolve the bundle-relative video to an absolute path so the
                    // webview's asset protocol can load it (M9-G).
                    let video = meta.video.map(|v| AttachedVideo {
                        path: bundle_dir.join(&v.file),
                        offset_us: v.offset_us,
                    });
                    (grid, key, origin, bpath, boffset, video, meta.backgrounds)
                }
                Err(_) => default_meta(),
            },
            Err(_) => default_meta(),
        };

    // Replace the composer.
    {
        let mut composer = state.composer.lock().expect("composer mutex poisoned");
        *composer = Composer::from_timeline(timeline, grid);
        composer.set_key(key);
        composer.set_backing_offset_us(backing_offset_us);
        composer.set_backgrounds(BackgroundStack::from_layers(backgrounds.clone()));
    }

    // Update the side-channel state.
    *state.key.lock().expect("key mutex poisoned") = key;
    *state.origin.lock().expect("origin mutex poisoned") = origin;
    *state.dirty.lock().expect("dirty mutex poisoned") = false;
    *state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned") = backing_path;
    *state.video.lock().expect("video mutex poisoned") = video;
    // Resolve each background layer's bundle-relative file to an absolute path
    // so the webview's asset protocol can load it (M14-D).
    *state
        .background_srcs
        .lock()
        .expect("background_srcs mutex poisoned") = backgrounds
        .into_iter()
        .map(|l| AttachedBackground {
            path: bundle_dir.join(&l.file),
            id: l.id,
        })
        .collect();
    // Remember where this piece came from so an in-place save overwrites it.
    *state
        .current_dir
        .lock()
        .expect("current_dir mutex poisoned") = Some(bundle_dir);

    // Return the fresh snapshot.
    let composer = state.composer.lock().expect("composer mutex poisoned");
    Ok(composer.snapshot())
}

/// Serializable backing-track reference for the edit screen — the absolute
/// `path` of the attached audio file and its display file name (no directory).
/// Mirrors the video-ref shape (M9-E). The alignment offset itself lives on the
/// composer (`backing_offset_us`) and surfaces in the snapshot, so it is not
/// duplicated here.
#[derive(Debug, Clone, Serialize)]
pub struct BackingRef {
    pub path: String,
    pub name: String,
}

/// Attach (or replace) the backing audio track on the live editor, marking the
/// timeline dirty so a later save persists it into the bundle's
/// `RecordingMeta.backing` (M9-E). `path` is the absolute file the webview
/// picked; the audio plumbing (attach to the playback thread) is driven by the
/// command wrapper in `lib.rs`, reusing the existing `attach_backing` path.
///
/// This is the sibling of [`set_video`]: it only owns the frontend-mirrored
/// state that `save_bundle` reads (`backing_path`); it does not touch the audio
/// device or the composer's `backing_offset_us`.
pub fn set_backing(state: &AppState, path: String) {
    *state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned") = Some(std::path::PathBuf::from(path));
    *state.dirty.lock().expect("dirty mutex poisoned") = true;
}

/// Detach the backing audio track. Marks the timeline dirty (M9-E). The audio
/// stop is driven by the command wrapper, reusing the existing `detach_backing`.
pub fn clear_backing(state: &AppState) {
    *state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned") = None;
    *state.dirty.lock().expect("dirty mutex poisoned") = true;
}

/// The currently attached backing track, or `None` (M9-E).
pub fn query_backing(state: &AppState) -> Option<BackingRef> {
    state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned")
        .as_ref()
        .map(|p| BackingRef {
            path: p.to_string_lossy().into_owned(),
            name: p
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| p.to_string_lossy().into_owned()),
        })
}

/// Serializable background-video reference for the edit screen — the absolute
/// `path` the webview wraps with `convertFileSrc`, plus the alignment offset.
#[derive(Debug, Clone, Serialize)]
pub struct VideoRef {
    pub path: String,
    pub offset_us: i64,
}

/// Attach (or replace) the background video, marking the timeline dirty so a
/// later save persists it into the bundle (M9-G). `path` is the absolute source
/// path the webview picked; `offset_us` is the alignment offset.
pub fn set_video(state: &AppState, path: String, offset_us: i64) {
    *state.video.lock().expect("video mutex poisoned") = Some(AttachedVideo {
        path: std::path::PathBuf::from(path),
        offset_us,
    });
    *state.dirty.lock().expect("dirty mutex poisoned") = true;
}

/// Update only the alignment offset of an already-attached video. No-op when no
/// video is attached. Marks the timeline dirty.
pub fn set_video_offset(state: &AppState, offset_us: i64) {
    let mut guard = state.video.lock().expect("video mutex poisoned");
    if let Some(v) = guard.as_mut() {
        v.offset_us = offset_us;
        drop(guard);
        *state.dirty.lock().expect("dirty mutex poisoned") = true;
    }
}

/// Detach the background video. Marks the timeline dirty.
pub fn clear_video(state: &AppState) {
    *state.video.lock().expect("video mutex poisoned") = None;
    *state.dirty.lock().expect("dirty mutex poisoned") = true;
}

/// The currently attached background video, or `None`.
pub fn query_video(state: &AppState) -> Option<VideoRef> {
    state
        .video
        .lock()
        .expect("video mutex poisoned")
        .as_ref()
        .map(|v| VideoRef {
            path: v.path.to_string_lossy().into_owned(),
            offset_us: v.offset_us,
        })
}

/// Serializable background-image reference for the edit/play screens: the
/// absolute `path` the webview wraps with `convertFileSrc`, plus the layer's
/// identity and its transform **already evaluated** at the playhead.
#[derive(Debug, Clone, Serialize)]
pub struct BackgroundRef {
    pub index: usize,
    pub id: String,
    pub file: String,
    pub path: String,
    pub selected: bool,
    pub transform: Transform,
    pub keyframes: Vec<Keyframe>,
}

/// Bundle-relative filename for a background image layer, derived from the
/// source extension (defaulting to `.png`) and the layer's ordinal.
fn background_bundle_filename(src: &std::path::Path, ordinal: usize) -> String {
    let ext = src
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty())
        .unwrap_or("png");
    format!("background-{ordinal}.{ext}")
}

/// Attach a background image by absolute path, appending it as the front-most
/// layer and selecting it (M14-D). Marks the timeline dirty so a later save
/// copies the file into the bundle and persists `meta.backgrounds`.
///
/// The new layer starts with **no keyframes** — a still, centred backdrop —
/// until an edit action writes the first one.
pub fn attach_background(state: &AppState, path: String) -> Vec<BackgroundRef> {
    let src = std::path::PathBuf::from(path);
    {
        let mut composer = state.composer.lock().expect("composer mutex poisoned");
        let stack = composer.backgrounds_mut();
        // Lowest free ordinal, so detaching then re-attaching reuses the slot
        // instead of growing the names forever.
        let ordinal = (0..)
            .find(|n| !stack.contains_id(&format!("bg-{n}")))
            .expect("free ordinal");
        let id = format!("bg-{ordinal}");
        let file = background_bundle_filename(&src, ordinal);
        stack.push(BackgroundImage::new(id.clone(), file));
        state
            .background_srcs
            .lock()
            .expect("background_srcs mutex poisoned")
            .push(AttachedBackground { id, path: src });
    }
    *state.dirty.lock().expect("dirty mutex poisoned") = true;
    query_backgrounds(state)
}

/// Detach the background layer with this id. Returns whether it existed; marks
/// the timeline dirty when it did.
pub fn detach_background(state: &AppState, id: &str) -> bool {
    let removed = {
        let mut composer = state.composer.lock().expect("composer mutex poisoned");
        composer.backgrounds_mut().remove_by_id(id).is_some()
    };
    if removed {
        state
            .background_srcs
            .lock()
            .expect("background_srcs mutex poisoned")
            .retain(|b| b.id != id);
        *state.dirty.lock().expect("dirty mutex poisoned") = true;
    }
    removed
}

/// Every background layer with its absolute source path and its transform
/// evaluated at the playhead — what the edit screen renders from.
pub fn query_backgrounds(state: &AppState) -> Vec<BackgroundRef> {
    let views = state
        .composer
        .lock()
        .expect("composer mutex poisoned")
        .background_views();
    let srcs = state
        .background_srcs
        .lock()
        .expect("background_srcs mutex poisoned");
    views
        .into_iter()
        .map(|v| {
            let path = srcs
                .iter()
                .find(|b| b.id == v.id)
                .map(|b| b.path.to_string_lossy().into_owned())
                .unwrap_or_default();
            BackgroundRef {
                index: v.index,
                id: v.id,
                file: v.file,
                path,
                selected: v.selected,
                transform: v.transform,
                keyframes: v.keyframes,
            }
        })
        .collect()
}

/// Current composer snapshot — mirrors `query state`.
pub fn query_state(state: &AppState) -> ComposerSnapshot {
    state
        .composer
        .lock()
        .expect("composer mutex poisoned")
        .snapshot()
}

/// Current dirty flag.
pub fn query_dirty(state: &AppState) -> bool {
    *state.dirty.lock().expect("dirty mutex poisoned")
}

/// The full, self-describing action catalog — mirrors `query help`.
///
/// Serialised from [`action_help`] so the webview can discover every action's
/// name, parameters, and description live (drift-proof).
pub fn query_help() -> serde_json::Value {
    serde_json::to_value(action_help()).expect("action_help serialises")
}

/// Advance the transport by `dt_us` microseconds **only while playing**, and
/// return any effects produced. The tick thread calls this with the measured
/// wall-clock delta; tests call it with a fixed delta.
///
/// When the composer is not playing this is a no-op returning no effects, so
/// the playhead never drifts while stopped.
pub fn tick_advance(state: &AppState, dt_us: u64) -> Vec<Effect> {
    let mut composer = state.composer.lock().expect("composer mutex poisoned");
    if !composer.is_playing() {
        return Vec::new();
    }
    composer.advance(dt_us)
}

/// Whether the composer transport is currently playing. Used to route synth
/// effects: during playback note-ons are polyphonic (many notes ring together),
/// while a stopped edit audition replaces the single previewed note.
pub fn is_playing(state: &AppState) -> bool {
    state
        .composer
        .lock()
        .expect("composer mutex poisoned")
        .is_playing()
}

/// Cheap transport read — `(advancing, playhead_us, backing_offset_us,
/// playback_rate)` — without building the full (all-notes) snapshot. The tick
/// thread calls this every tick for backing sync and the lightweight playhead
/// push, so the ~900-note snapshot is only serialised when the notes actually
/// change, not 250×/second.
///
/// The first field is [`Composer::is_advancing`], not `is_playing`: a wait-mode
/// freeze reports as **not advancing** so the backing audio pauses and the
/// playhead push stops, exactly like a manual pause. The full snapshot still
/// reports `playing = true` (with `frozen = true`) so the highway stays anchored
/// on the playhead.
pub fn transport_fields(state: &AppState) -> (bool, u64, u64, f64) {
    let composer = state.composer.lock().expect("composer mutex poisoned");
    (
        composer.is_advancing(),
        composer.playhead_us(),
        composer.backing_offset_us(),
        composer.playback_rate(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn run_action_add_note_adds_one_note_at_cursor() {
        let state = AppState::new();
        let before = query_state(&state);
        assert!(before.notes.is_empty());

        let reply = run_action(&state, "add_note", &json!({})).expect("add_note applies");
        assert_eq!(reply.snapshot.notes.len(), 1);
        // The note sits at the cursor's pitch.
        assert_eq!(reply.snapshot.notes[0].pitch, before.cursor.pitch);
    }

    /// Saving a bundle back over itself must succeed even though its backing
    /// track already lives in the destination — the normal case for an imported
    /// piece. Regression: the copy was source==destination, which Windows
    /// rejects, aborting the save after song.mid was written but before
    /// meta.json and leaving the piece dirty on disk.
    #[test]
    fn write_bundle_in_place_with_backing_already_in_bundle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bundle = tmp.path().join("piece");
        std::fs::create_dir_all(&bundle).unwrap();
        let backing = bundle.join("backing.wav");
        std::fs::write(&backing, b"audio-bytes").unwrap();

        write_bundle(
            &bundle,
            &Timeline::default(),
            Grid::default_120(),
            Key {
                root_pc: 0,
                scale: rockcraft_core::Scale::Major,
            },
            TrackOrigin::Imported,
            Some(backing.as_path()), // already inside `bundle`
            0,
            None,
            &[],
            &[],
        )
        .expect("in-place save must not fail on a self-copy");

        // meta.json is written (the step that used to be skipped) and still
        // references the untouched backing file.
        let meta =
            RecordingMeta::from_json(&std::fs::read_to_string(bundle.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta.backing.map(|b| b.file).as_deref(), Some("backing.wav"));
        assert_eq!(std::fs::read(&backing).unwrap(), b"audio-bytes");
        assert!(bundle.join("song.mid").exists());
    }

    /// The same in-place save, but with the bundle addressed by a *relative*
    /// path while the backing is tracked absolutely — exactly how a library save
    /// arrives, since `library_root()` is relative and attached media is not.
    /// A textual path comparison misses this and self-copies; only resolving
    /// both sides catches it.
    #[test]
    fn write_bundle_in_place_survives_relative_bundle_dir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let bundle_abs = tmp.path().join("piece");
        std::fs::create_dir_all(&bundle_abs).unwrap();
        let backing_abs = bundle_abs.join("backing.wav");
        std::fs::write(&backing_abs, b"audio-bytes").unwrap();

        // Address the bundle relatively, from inside the tempdir.
        let prev_cwd = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let result = write_bundle(
            std::path::Path::new("piece"), // relative
            &Timeline::default(),
            Grid::default_120(),
            Key {
                root_pc: 0,
                scale: rockcraft_core::Scale::Major,
            },
            TrackOrigin::Imported,
            Some(backing_abs.as_path()), // absolute, same file
            0,
            None,
            &[],
            &[],
        );
        std::env::set_current_dir(prev_cwd).unwrap();

        result.expect("relative bundle dir must not defeat the self-copy guard");
        assert_eq!(std::fs::read(&backing_abs).unwrap(), b"audio-bytes");
        assert!(bundle_abs.join("meta.json").exists());
    }

    #[test]
    fn is_same_file_resolves_relative_and_absolute() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let f = tmp.path().join("a.bin");
        std::fs::write(&f, b"x").unwrap();
        let prev = std::env::current_dir().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();
        let same = is_same_file(&f, std::path::Path::new("a.bin"));
        let differs = is_same_file(&f, std::path::Path::new("b.bin"));
        std::env::set_current_dir(prev).unwrap();
        assert!(same, "same file via absolute and relative paths");
        assert!(!differs, "a missing sibling is not the same file");
    }

    #[test]
    fn run_action_resize_note_grows_duration() {
        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note applies");
        let added = query_state(&state);
        let dur_before = added.notes[0].dur_us;

        let reply = run_action(&state, "resize_note", &json!({ "delta_steps": 2 }))
            .expect("resize_note applies");
        assert_eq!(reply.snapshot.notes.len(), 1);
        assert!(
            reply.snapshot.notes[0].dur_us > dur_before,
            "duration should grow after a positive resize"
        );
    }

    #[test]
    fn run_action_unknown_name_errors() {
        let state = AppState::new();
        let err = run_action(&state, "frobnicate", &json!({})).unwrap_err();
        assert!(
            err.contains("unknown action"),
            "error should mention unknown action, got: {err}"
        );
    }

    #[test]
    fn run_action_bad_params_errors() {
        let state = AppState::new();
        let err = run_action(&state, "resize_note", &json!({ "delta_steps": "lots" })).unwrap_err();
        assert!(
            err.contains("bad params"),
            "error should mention bad params, got: {err}"
        );
    }

    #[test]
    fn tick_advance_moves_playhead_only_while_playing() {
        let state = AppState::new();

        // Stopped: advancing does nothing.
        assert_eq!(query_state(&state).playhead_us, 0);
        let effects = tick_advance(&state, 100_000);
        assert!(effects.is_empty());
        assert_eq!(
            query_state(&state).playhead_us,
            0,
            "playhead must not move while stopped"
        );

        // Playing from start: advancing moves the playhead.
        run_action(&state, "play_from_start", &json!({})).expect("play_from_start applies");
        assert!(query_state(&state).playing);
        tick_advance(&state, 100_000);
        assert_eq!(
            query_state(&state).playhead_us,
            100_000,
            "playhead must advance by dt_us while playing"
        );
    }

    #[test]
    fn query_help_lists_known_actions() {
        let help = query_help();
        let arr = help.as_array().expect("help is a JSON array");
        assert!(!arr.is_empty());
        let names: Vec<&str> = arr
            .iter()
            .filter_map(|info| info.get("name").and_then(|n| n.as_str()))
            .collect();
        assert!(names.contains(&"add_note"));
        assert!(names.contains(&"resize_note"));
    }

    #[test]
    fn add_note_marks_dirty() {
        let state = AppState::new();
        assert!(!query_dirty(&state), "fresh state is not dirty");
        run_action(&state, "add_note", &json!({})).expect("add_note applies");
        assert!(query_dirty(&state), "add_note must mark dirty");
    }

    #[test]
    fn save_bundle_round_trip() {
        // Build a composer with two notes.
        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note 1");
        run_action(&state, "cursor_right", &json!({})).expect("cursor_right");
        run_action(&state, "cursor_up", &json!({})).expect("cursor_up");
        run_action(&state, "add_note", &json!({})).expect("add_note 2");

        let snap_before = query_state(&state);
        assert_eq!(snap_before.notes.len(), 2);

        // Write the bundle directly into a temp directory (bypasses library root).
        let bundle_dir = std::env::temp_dir().join(format!(
            "rockcraft-rt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        {
            let composer = state.composer.lock().unwrap();
            write_bundle(
                &bundle_dir,
                composer.timeline(),
                composer.grid(),
                *state.key.lock().unwrap(),
                *state.origin.lock().unwrap(),
                None,
                0,
                None,
                &[],
                &[],
            )
            .expect("write_bundle should succeed");
        }
        // Mark clean so we can test dirty after load.
        *state.dirty.lock().unwrap() = false;

        // Verify meta.json deserializes as RecordingMeta.
        let meta_json =
            std::fs::read_to_string(bundle_dir.join("meta.json")).expect("meta.json should exist");
        let meta = RecordingMeta::from_json(&meta_json).expect("meta.json should parse");
        assert_eq!(meta.midi_file, "song.mid");
        assert!(meta.grid.is_some(), "meta should carry grid");
        assert!(meta.key.is_some(), "meta should carry key");

        // Load back and check round-trip.
        let dir_str = bundle_dir.to_string_lossy().into_owned();
        let snap_after = load_bundle(&state, &dir_str).expect("load_bundle should succeed");
        assert_eq!(
            snap_after.notes.len(),
            snap_before.notes.len(),
            "loaded note count must match saved note count"
        );
        let mut pitches_before: Vec<u8> = snap_before.notes.iter().map(|n| n.pitch).collect();
        let mut pitches_after: Vec<u8> = snap_after.notes.iter().map(|n| n.pitch).collect();
        pitches_before.sort_unstable();
        pitches_after.sort_unstable();
        assert_eq!(pitches_before, pitches_after, "pitches must round-trip");

        // Dirty cleared after load.
        assert!(!query_dirty(&state), "dirty flag must be clear after load");

        // Cleanup.
        let _ = std::fs::remove_dir_all(&bundle_dir);
    }

    #[test]
    fn legacy_load_no_meta_uses_defaults() {
        // A bundle with only song.mid (no meta.json) should load with default grid.
        let tmp = std::env::temp_dir().join(format!(
            "rockcraft-legacy-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&tmp).unwrap();

        // Write a minimal MIDI file (just a note-on/off pair).
        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note");
        let composer = state.composer.lock().unwrap();
        let bytes = rockcraft_midi::events_to_smf_bytes(&composer.timeline().to_events());
        drop(composer);
        std::fs::write(tmp.join("song.mid"), bytes).unwrap();
        // No meta.json written.

        // Load should succeed with default grid.
        let snap = load_bundle(&state, &tmp.to_string_lossy()).expect("legacy load should succeed");
        assert_eq!(snap.notes.len(), 1, "legacy bundle note count");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// M9-E: a backing track attached while editing (`set_backing`) is written
    /// into the bundle's `meta.json` on save and restored into `backing_path`
    /// (and the composer's offset) on load — the relocation's persistence
    /// round-trip. Mirrors the video round-trip but for audio backing.
    #[test]
    fn set_backing_persists_and_round_trips() {
        let dir = std::env::temp_dir().join(format!(
            "rockcraft-backing-rt-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // A stand-in backing file the picker would have chosen.
        let src_backing = dir.join("my-song.ogg");
        std::fs::write(&src_backing, b"not really audio, just bytes").unwrap();

        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note");

        // Attach the backing while editing, then give it a non-zero alignment
        // offset (the existing nudge path the spec must not break).
        set_backing(&state, src_backing.to_string_lossy().into_owned());
        assert!(query_dirty(&state), "attaching backing marks dirty");
        let backing_ref = query_backing(&state).expect("backing attached");
        assert_eq!(backing_ref.name, "my-song.ogg");
        run_action(
            &state,
            "nudge_backing_offset",
            &json!({ "delta_us": 250_000 }),
        )
        .expect("nudge_backing_offset");

        // Save into a bundle directory (write_bundle reads backing_path + offset).
        let bundle_dir = dir.join("bundle");
        {
            let composer = state.composer.lock().unwrap();
            write_bundle(
                &bundle_dir,
                composer.timeline(),
                composer.grid(),
                *state.key.lock().unwrap(),
                *state.origin.lock().unwrap(),
                state.backing_path.lock().unwrap().as_deref(),
                composer.backing_offset_us(),
                None,
                &[],
                &[],
            )
            .expect("write_bundle with backing should succeed");
        }

        // meta.json carries the backing entry (bundle-relative filename + offset).
        let meta_json =
            std::fs::read_to_string(bundle_dir.join("meta.json")).expect("meta.json exists");
        let meta = RecordingMeta::from_json(&meta_json).expect("meta parses");
        let backing = meta.backing.expect("meta.backing is present after save");
        assert_eq!(backing.file, "backing.ogg", "backing filename in meta");
        assert_eq!(
            backing.audio_start_us, 250_000,
            "backing alignment offset round-trips into meta"
        );
        assert!(
            bundle_dir.join("backing.ogg").exists(),
            "backing audio copied into the bundle"
        );

        // Load the bundle back: backing_path and the composer offset are restored.
        let snap = load_bundle(&state, &bundle_dir.to_string_lossy())
            .expect("load_bundle with backing succeeds");
        assert_eq!(
            snap.backing_offset_us, 250_000,
            "composer backing offset restored on load"
        );
        let reloaded = query_backing(&state).expect("backing restored after load");
        assert_eq!(reloaded.name, "backing.ogg");
        assert!(
            reloaded.path.ends_with("backing.ogg"),
            "restored backing path points into the bundle, got {}",
            reloaded.path
        );

        // Detach clears it and marks dirty again.
        clear_backing(&state);
        assert!(query_backing(&state).is_none(), "detach clears backing");
        assert!(query_dirty(&state), "detach marks dirty");

        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── background images (M14-D) ──────────────────────────────────────────

    /// A unique temp directory for a bundle round-trip test.
    fn temp_dir(tag: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rockcraft-{tag}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn attach_background_adds_a_selected_still_layer() {
        let state = AppState::new();
        assert!(query_backgrounds(&state).is_empty());

        let layers = attach_background(&state, "/tmp/art.jpg".to_string());
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].id, "bg-0");
        assert_eq!(layers[0].file, "background-0.jpg");
        assert_eq!(layers[0].path, "/tmp/art.jpg");
        assert!(layers[0].selected);
        // A new layer starts still — no keyframes, identity transform.
        assert!(layers[0].keyframes.is_empty());
        assert_eq!(layers[0].transform, Transform::IDENTITY);
        assert!(query_dirty(&state), "attaching marks the piece dirty");

        // A second layer goes in front and takes the selection.
        let layers = attach_background(&state, "/tmp/second.png".to_string());
        assert_eq!(layers.len(), 2);
        assert_eq!(layers[1].id, "bg-1");
        assert_eq!(layers[1].file, "background-1.png");
        assert!(layers[1].selected && !layers[0].selected);
        // …and the snapshot agrees.
        assert_eq!(query_state(&state).selected_background, Some(1));
    }

    #[test]
    fn background_actions_animate_the_selected_layer() {
        let state = AppState::new();
        attach_background(&state, "/tmp/art.png".to_string());

        // Keyframe the identity at t=0, then pan at a later playhead.
        run_action(&state, "add_background_keyframe", &json!({})).expect("keyframe");
        run_action(&state, "cursor_bar_right", &json!({})).expect("cursor");
        run_action(
            &state,
            "nudge_background_pos",
            &json!({ "dx_permille": 500, "dy_permille": 0 }),
        )
        .expect("pan");

        let layers = query_backgrounds(&state);
        assert_eq!(layers[0].keyframes.len(), 2);
        // The playhead sits on the far keyframe, so the layer is fully panned.
        assert!((layers[0].transform.x - 0.5).abs() < 1e-5);
    }

    #[test]
    fn detach_background_removes_the_layer_and_reuses_its_slot() {
        let state = AppState::new();
        attach_background(&state, "/tmp/a.png".to_string());
        attach_background(&state, "/tmp/b.png".to_string());
        assert!(!detach_background(&state, "bg-9"), "unknown id");
        assert!(detach_background(&state, "bg-0"));

        let layers = query_backgrounds(&state);
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0].id, "bg-1");
        // The freed ordinal is reused rather than growing forever.
        let layers = attach_background(&state, "/tmp/c.png".to_string());
        assert_eq!(layers[1].id, "bg-0");
    }

    #[test]
    fn backgrounds_round_trip_through_a_saved_bundle() {
        let dir = temp_dir("backgrounds");
        let src = dir.join("art.png");
        std::fs::write(&src, b"PNG").unwrap();

        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note");
        attach_background(&state, src.to_string_lossy().into_owned());
        // Animate: identity at 0, panned + rotated a bar later.
        run_action(&state, "add_background_keyframe", &json!({})).expect("keyframe");
        run_action(&state, "cursor_bar_right", &json!({})).expect("cursor");
        run_action(
            &state,
            "nudge_background_pos",
            &json!({ "dx_permille": 250, "dy_permille": -125 }),
        )
        .expect("pan");
        run_action(
            &state,
            "nudge_background_rotation",
            &json!({ "delta_millideg": 15_000 }),
        )
        .expect("rotate");
        let before = query_backgrounds(&state);

        let bundle = dir.join("bundle");
        save_bundle_into(&state, &bundle).expect("save");
        assert!(
            bundle.join("background-0.png").exists(),
            "the image is copied into the bundle"
        );
        let meta =
            RecordingMeta::from_json(&std::fs::read_to_string(bundle.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(meta.backgrounds.len(), 1);
        assert_eq!(meta.backgrounds[0].keyframes.len(), 2);

        // Loading restores the animation *and* re-points the layer at the copy
        // inside the bundle, so the webview can load it.
        let fresh = AppState::new();
        load_bundle(&fresh, &bundle.to_string_lossy()).expect("load");
        let after = query_backgrounds(&fresh);
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].keyframes, before[0].keyframes);
        assert_eq!(
            after[0].path,
            bundle.join("background-0.png").to_string_lossy()
        );

        // Re-saving the loaded piece is a no-op copy, not an error.
        let bundle2 = dir.join("bundle2");
        save_bundle_into(&fresh, &bundle2).expect("re-save");
        assert!(bundle2.join("background-0.png").exists());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn split_carries_backgrounds_into_each_part() {
        let dir = temp_dir("split-backgrounds");
        let src = dir.join("art.png");
        std::fs::write(&src, b"PNG").unwrap();

        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note");
        attach_background(&state, src.to_string_lossy().into_owned());
        // A 4 s pan across the whole piece.
        run_action(&state, "add_background_keyframe", &json!({})).expect("keyframe at 0");
        run_action(&state, "set_playhead", &json!({ "us": 4_000_000 })).expect("playhead");
        run_action(&state, "play", &json!({ "from_us": 4_000_000 })).expect("play");
        run_action(
            &state,
            "nudge_background_pos",
            &json!({ "dx_permille": 1_000, "dy_permille": 0 }),
        )
        .expect("pan");
        run_action(&state, "stop", &json!({})).expect("stop");

        let root = dir.join("library");
        let dirs = split_bundle_into(
            &state,
            &root,
            vec![SplitSegment {
                start_us: 1_000_000,
                end_us: 3_000_000,
                name: "Middle".into(),
            }],
        )
        .expect("split");
        assert_eq!(dirs.len(), 1);
        let part = std::path::PathBuf::from(&dirs[0]);
        assert!(part.join("background-0.png").exists(), "image copied in");
        let meta =
            RecordingMeta::from_json(&std::fs::read_to_string(part.join("meta.json")).unwrap())
                .unwrap();
        let layer = &meta.backgrounds[0];
        // The part opens and closes on the layout the whole piece had there.
        assert!((layer.transform_at(0).x - 0.25).abs() < 1e-5);
        assert!((layer.transform_at(2_000_000).x - 0.75).abs() < 1e-5);

        std::fs::remove_dir_all(&dir).ok();
    }

    /// M10-E: swapping or detaching the backing audio of a piece that carries a
    /// background video must preserve `meta.video` across a save → load
    /// round-trip. Backing and video live in independent state, so mutating one
    /// must never clear the other.
    #[test]
    fn backing_swap_preserves_video() {
        let dir = std::env::temp_dir().join(format!(
            "rockcraft-swap-video-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let src_video = dir.join("clip.mp4");
        let backing_a = dir.join("source-audio.ogg");
        let backing_b = dir.join("studio.flac");
        std::fs::write(&src_video, b"VIDEO").unwrap();
        std::fs::write(&backing_a, b"AUDIO A").unwrap();
        std::fs::write(&backing_b, b"AUDIO B").unwrap();

        // Save the current state into `bundle`, mirroring `save_bundle`'s reads.
        let do_save = |state: &AppState, bundle: &std::path::Path| {
            let composer = state.composer.lock().unwrap();
            let timeline = composer.timeline().clone();
            let grid = composer.grid();
            let offset = composer.backing_offset_us();
            drop(composer);
            let key = *state.key.lock().unwrap();
            let origin = *state.origin.lock().unwrap();
            let backing_path = state.backing_path.lock().unwrap().clone();
            let video = state.video.lock().unwrap().clone();
            write_bundle(
                bundle,
                &timeline,
                grid,
                key,
                origin,
                backing_path.as_deref(),
                offset,
                video.as_ref(),
                &[],
                &[],
            )
            .expect("write_bundle should succeed");
        };

        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note");
        set_video(&state, src_video.to_string_lossy().into_owned(), -100_000);
        set_backing(&state, backing_a.to_string_lossy().into_owned());

        // Persist the piece with both media, then load it back so the state now
        // references the bundle-local copies (the realistic edit entry point).
        let bundle1 = dir.join("bundle1");
        do_save(&state, &bundle1);
        load_bundle(&state, &bundle1.to_string_lossy()).expect("load bundle1");
        let v = query_video(&state).expect("video restored on load");
        assert_eq!(v.offset_us, -100_000);
        assert!(query_backing(&state).is_some(), "backing restored on load");

        // Swap the backing for a different file and save again.
        set_backing(&state, backing_b.to_string_lossy().into_owned());
        let bundle2 = dir.join("bundle2");
        do_save(&state, &bundle2);
        let meta2 =
            RecordingMeta::from_json(&std::fs::read_to_string(bundle2.join("meta.json")).unwrap())
                .unwrap();
        assert_eq!(
            meta2.backing.expect("swapped backing present").file,
            "backing.flac",
            "the new backing file is saved"
        );
        let video2 = meta2.video.expect("video preserved through backing swap");
        assert_eq!(video2.file, "background.mp4");
        assert_eq!(video2.offset_us, -100_000);
        assert!(
            bundle2.join("background.mp4").exists(),
            "video copied along"
        );

        // Reload the swapped bundle: the video reference is still intact.
        load_bundle(&state, &bundle2.to_string_lossy()).expect("load bundle2");
        assert_eq!(
            query_video(&state).expect("video still present").offset_us,
            -100_000
        );

        // Detaching the backing keeps the video as well.
        clear_backing(&state);
        let bundle3 = dir.join("bundle3");
        do_save(&state, &bundle3);
        let meta3 =
            RecordingMeta::from_json(&std::fs::read_to_string(bundle3.join("meta.json")).unwrap())
                .unwrap();
        assert!(meta3.backing.is_none(), "detach drops the backing");
        let video3 = meta3.video.expect("video preserved through detach");
        assert_eq!(video3.file, "background.mp4");
        assert_eq!(video3.offset_us, -100_000);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// M10-B: `split_bundle` writes each kept segment as its own library bundle
    /// with copied media + derived offsets and `origin = Edited`, leaving the
    /// source piece and its media untouched.
    #[test]
    fn split_bundle_writes_kept_parts_with_copied_media() {
        let dir = std::env::temp_dir().join(format!(
            "rockcraft-split-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        // Stand-in source media the editor has attached.
        let src_backing = dir.join("my-song.ogg");
        let src_video = dir.join("clip.mp4");
        std::fs::write(&src_backing, b"AUDIO").unwrap();
        std::fs::write(&src_video, b"VIDEO").unwrap();

        let state = AppState::new();
        run_action(&state, "add_note", &json!({})).expect("add_note");
        set_backing(&state, src_backing.to_string_lossy().into_owned());
        run_action(
            &state,
            "nudge_backing_offset",
            &json!({ "delta_us": 250_000 }),
        )
        .expect("nudge_backing_offset");
        set_video(&state, src_video.to_string_lossy().into_owned(), -100_000);

        let lib = dir.join("library");
        let segs = vec![
            SplitSegment {
                start_us: 0,
                end_us: 1_000_000,
                name: "Part One".into(),
            },
            SplitSegment {
                start_us: 2_000_000,
                end_us: 3_000_000,
                name: "Part Two".into(),
            },
        ];
        let dirs = split_bundle_into(&state, &lib, segs).expect("split_bundle succeeds");
        assert_eq!(dirs.len(), 2, "one bundle per kept segment");

        for (path, seg_start) in dirs.iter().zip([0u64, 2_000_000]) {
            let bundle = std::path::Path::new(path);
            assert!(bundle.join("song.mid").exists(), "song.mid written");
            assert_eq!(std::fs::read(bundle.join("backing.ogg")).unwrap(), b"AUDIO");
            assert_eq!(
                std::fs::read(bundle.join("background.mp4")).unwrap(),
                b"VIDEO"
            );

            let meta = RecordingMeta::from_json(
                &std::fs::read_to_string(bundle.join("meta.json")).unwrap(),
            )
            .unwrap();
            assert_eq!(meta.origin, Some(TrackOrigin::Edited));
            assert!(meta.grid.is_some() && meta.key.is_some());
            assert_eq!(meta.backing.unwrap().audio_start_us, 250_000 + seg_start);
            assert_eq!(meta.video.unwrap().offset_us, -100_000 + seg_start as i64);
        }

        // The source media is left untouched (non-destructive).
        assert_eq!(std::fs::read(&src_backing).unwrap(), b"AUDIO");
        assert_eq!(std::fs::read(&src_video).unwrap(), b"VIDEO");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn split_bundle_rejects_empty_segments() {
        let state = AppState::new();
        let err = split_bundle_into(&state, std::path::Path::new("/tmp/x"), vec![]).unwrap_err();
        assert!(err.contains("no segments"), "got: {err}");
    }
}
