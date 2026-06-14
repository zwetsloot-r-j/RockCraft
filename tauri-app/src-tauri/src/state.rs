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
    action_from_name, action_help, ActionError, BackgroundVideo, BackingTrack, Composer,
    ComposerSnapshot, Effect, Grid, Key, NoteView, RecordingMeta, Scale, Timeline, TrackOrigin,
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
}
