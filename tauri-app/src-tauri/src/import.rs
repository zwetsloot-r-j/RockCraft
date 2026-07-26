//! Import flow — backend commands for the Tauri import screen.
//!
//! Exposes two Tauri commands:
//! - [`import_url_available`]: returns whether a fetch command is configured.
//! - [`import_start`]: spawns a background thread running the import pipeline,
//!   emitting `import_progress` events to the webview as the pipeline advances.
//!
//! The `import_progress` event payload is a JSON object:
//! ```json
//! { "stage": "fetching" | "extracting" | "writing" | "done" | "failed",
//!   "progress": 0.0..1.0,   // only meaningful for "extracting"
//!   "log": "...",            // a single log line from the pipeline
//!   "bundle_dir": "...",     // absolute path, only on "done"
//!   "error": "..."           // only on "failed"
//! }
//! ```

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rockcraft_import::{fetch_command_configured, import_source, ImportInput, Progress};
use serde::Serialize;
use tauri::{AppHandle, Emitter, State};

/// Event name emitted to the webview for each pipeline progress step.
const EVENT_IMPORT_PROGRESS: &str = "import_progress";

/// Atomic flag: `true` while an import is in progress.
pub struct ImportRunning(pub Arc<AtomicBool>);

impl Default for ImportRunning {
    fn default() -> Self {
        Self(Arc::new(AtomicBool::new(false)))
    }
}

/// Serialisable progress event payload sent to the webview.
#[derive(Debug, Clone, Serialize)]
pub struct ImportProgressEvent {
    pub stage: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_dir: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Input DTO from the frontend — mirrors `ImportInput` on the Rust side.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum ImportInputDto {
    File(String),
    Url(String),
    /// A local digital score file (MusicXML and friends) — the M13-A path.
    Score(String),
}

/// Map a `Progress` event to an `ImportProgressEvent` given the current stage.
fn progress_to_event(p: Progress, current_stage: &mut String) -> ImportProgressEvent {
    match p {
        Progress::Fetching => {
            *current_stage = "fetching".to_string();
            ImportProgressEvent {
                stage: "fetching".to_string(),
                progress: None,
                log: None,
                bundle_dir: None,
                error: None,
            }
        }
        Progress::Log(line) => ImportProgressEvent {
            stage: current_stage.clone(),
            progress: None,
            log: Some(line),
            bundle_dir: None,
            error: None,
        },
        Progress::Extracting(f) => {
            *current_stage = "extracting".to_string();
            ImportProgressEvent {
                stage: "extracting".to_string(),
                progress: Some(f),
                log: None,
                bundle_dir: None,
                error: None,
            }
        }
        Progress::Writing => {
            *current_stage = "writing".to_string();
            ImportProgressEvent {
                stage: "writing".to_string(),
                progress: None,
                log: None,
                bundle_dir: None,
                error: None,
            }
        }
        Progress::Done(path) => {
            *current_stage = "done".to_string();
            ImportProgressEvent {
                stage: "done".to_string(),
                progress: None,
                log: None,
                bundle_dir: Some(path.to_string_lossy().into_owned()),
                error: None,
            }
        }
    }
}

/// Returns `true` when a fetch command is configured for URL imports.
#[tauri::command]
pub fn import_url_available() -> bool {
    fetch_command_configured()
}

/// Start an import in the background. Returns `Err("import already running")`
/// when a concurrent import is in progress; otherwise spawns a thread and
/// returns `Ok(())` immediately.
#[tauri::command]
pub fn import_start(
    app: AppHandle,
    state: State<'_, ImportRunning>,
    input: ImportInputDto,
) -> Result<(), String> {
    let flag = Arc::clone(&state.0);
    // CAS: false → true. If it was already true, reject.
    if flag
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err("import already running".to_string());
    }

    let import_input = match input {
        ImportInputDto::File(p) => ImportInput::File(std::path::PathBuf::from(p)),
        ImportInputDto::Url(u) => ImportInput::Url(u),
        ImportInputDto::Score(p) => ImportInput::Score(std::path::PathBuf::from(p)),
    };

    std::thread::spawn(move || {
        let mut current_stage = String::from("fetching");

        let result = import_source(import_input, &mut |p| {
            let event = progress_to_event(p, &mut current_stage);
            let _ = app.emit(EVENT_IMPORT_PROGRESS, &event);
        });

        match result {
            Ok(_) => {
                // Done event is already emitted inside the progress callback.
            }
            Err(e) => {
                let _ = app.emit(
                    EVENT_IMPORT_PROGRESS,
                    &ImportProgressEvent {
                        stage: "failed".to_string(),
                        progress: None,
                        log: None,
                        bundle_dir: None,
                        error: Some(e.to_string()),
                    },
                );
            }
        }

        // Release the slot.
        flag.store(false, Ordering::SeqCst);
    });

    Ok(())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Concurrency guard: a second `compare_exchange(false, true)` fails while
    /// the flag is already `true`. This exercises the same CAS logic as
    /// `import_start` without needing a Tauri `AppHandle`.
    #[test]
    fn concurrency_guard_rejects_second_import() {
        let flag = Arc::new(AtomicBool::new(false));

        // First "import" acquires the slot.
        let res1 = flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(res1.is_ok(), "first acquire should succeed");

        // Second "import" must be rejected while the first holds the flag.
        let flag2 = Arc::clone(&flag);
        let res2 = flag2.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(
            res2.is_err(),
            "second acquire must fail while first holds slot"
        );

        // After releasing, a third acquire succeeds.
        flag.store(false, Ordering::SeqCst);
        let res3 = flag.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst);
        assert!(res3.is_ok(), "acquire after release must succeed");
    }

    /// Each `Progress` variant must serialise to the documented event payload.
    #[test]
    fn progress_to_event_mapping() {
        let mut stage = "fetching".to_string();

        // Fetching
        let ev = progress_to_event(Progress::Fetching, &mut stage);
        assert_eq!(ev.stage, "fetching");
        assert!(ev.progress.is_none());
        assert!(ev.log.is_none());

        // Log line — stays on the current stage
        let ev = progress_to_event(Progress::Log("line 1".to_string()), &mut stage);
        assert_eq!(ev.stage, "fetching");
        assert_eq!(ev.log.as_deref(), Some("line 1"));

        // Extracting
        let ev = progress_to_event(Progress::Extracting(0.5), &mut stage);
        assert_eq!(ev.stage, "extracting");
        assert_eq!(ev.progress, Some(0.5));

        // Writing
        let ev = progress_to_event(Progress::Writing, &mut stage);
        assert_eq!(ev.stage, "writing");

        // Done
        let bundle = std::path::PathBuf::from("/tmp/bundle-dir");
        let ev = progress_to_event(Progress::Done(bundle.clone()), &mut stage);
        assert_eq!(ev.stage, "done");
        assert_eq!(ev.bundle_dir.as_deref(), Some("/tmp/bundle-dir"));
        assert!(ev.error.is_none());
    }

    /// Each `ImportProgressEvent` must serialise with only the relevant fields
    /// present (none of the `skip_serializing_if` fields should appear when None).
    #[test]
    fn event_serialises_without_nulls() {
        let ev = ImportProgressEvent {
            stage: "extracting".to_string(),
            progress: Some(0.75),
            log: None,
            bundle_dir: None,
            error: None,
        };
        let json = serde_json::to_string(&ev).unwrap();
        assert!(json.contains("\"stage\":\"extracting\""));
        assert!(json.contains("\"progress\":0.75"));
        assert!(
            !json.contains("\"log\""),
            "None fields must not appear: {json}"
        );
        assert!(
            !json.contains("\"bundle_dir\""),
            "None fields must not appear: {json}"
        );
        assert!(
            !json.contains("\"error\""),
            "None fields must not appear: {json}"
        );
    }
}
