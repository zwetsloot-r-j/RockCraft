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
    action_from_name, action_help, slice_segment, ActionError, BackgroundVideo, BackingTrack,
    Composer, ComposerSnapshot, Effect, Grid, Key, NoteView, RecordingMeta, Scale, Segment,
    Timeline, TrackOrigin,
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
    let composer = state.composer.lock().expect("composer mutex poisoned");
    let timeline = composer.timeline().clone();
    let grid = composer.grid();
    let backing_offset_us = composer.backing_offset_us();
    drop(composer);

    let key = *state.key.lock().expect("key mutex poisoned");
    let origin = *state.origin.lock().expect("origin mutex poisoned");
    let backing_path = state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned")
        .clone();
    let video = state.video.lock().expect("video mutex poisoned").clone();

    let bundle_dir = match &dest {
        SaveDest::QuickSave => {
            let stamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            std::path::PathBuf::from("recordings").join(format!("take-{stamp}"))
        }
        SaveDest::Library { name } => {
            let slug = rockcraft_midi::bundle::slug(name);
            if slug.is_empty() {
                return Err("empty name — cannot save".to_string());
            }
            rockcraft_midi::bundle::library_root().join(slug)
        }
    };

    write_bundle(
        &bundle_dir,
        &timeline,
        grid,
        key,
        origin,
        backing_path.as_deref(),
        backing_offset_us,
        video.as_ref(),
    )
    .map_err(|e| e.to_string())?;

    // Clear dirty flag after a successful save.
    let mut dirty = state.dirty.lock().expect("dirty mutex poisoned");
    *dirty = false;

    Ok(bundle_dir.to_string_lossy().into_owned())
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
) -> std::io::Result<()> {
    std::fs::create_dir_all(bundle_dir)?;
    let bytes = events_to_smf_bytes(&timeline.to_events());
    std::fs::write(bundle_dir.join("song.mid"), bytes)?;

    let backing = if let Some(bpath) = backing_path {
        let filename = crate::record_bundle_backing_filename(bpath);
        std::fs::copy(bpath, bundle_dir.join(&filename))?;
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

    let meta = RecordingMeta {
        midi_file: "song.mid".into(),
        backing,
        grid: Some(grid),
        key: Some(key),
        origin: Some(origin),
        video,
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
    drop(composer);

    let key = *state.key.lock().expect("key mutex poisoned");
    let backing_path = state
        .backing_path
        .lock()
        .expect("backing_path mutex poisoned")
        .clone();
    let video = state.video.lock().expect("video mutex poisoned").clone();

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
        );
        let dir = library_root.join(&slug);
        rockcraft_import::write_part_bundle(
            &dir,
            &sliced,
            grid,
            key,
            backing_path.as_deref(),
            video.as_ref().map(|v| v.path.as_path()),
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
    let already_in_bundle = v.path.parent() == Some(bundle_dir);
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
        )
    };
    let (grid, key, origin, backing_path, backing_offset_us, video) =
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
                    (grid, key, origin, bpath, boffset, video)
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
        let mut pitches_before: Vec<u8> = snap_before.notes.iter().map(|n| n.pitch as u8).collect();
        let mut pitches_after: Vec<u8> = snap_after.notes.iter().map(|n| n.pitch as u8).collect();
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
