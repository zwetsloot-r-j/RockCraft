use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use rockcraft_core::{BackgroundVideo, BackingTrack};

use crate::{error::ImportError, parser::from_json, writer::write_chart_bundle_full};

/// Bundle-relative filename of the audio extracted from the source video.
const BACKING_FILENAME: &str = "backing.wav";

/// Bundle-relative base name (without extension) for the retained source video.
const VIDEO_BASENAME: &str = "source";

/// Frame rate every retained backdrop is normalised to (see
/// [`transcode_backdrop`]). A backdrop is scrubbed and watched, never scored
/// against — 30 fps halves the decode work of a 60 fps source for no loss that
/// matters here.
const BACKDROP_FPS: &str = "30";
/// Keyframe interval, in frames, for a retained backdrop — 0.5 s at
/// [`BACKDROP_FPS`]. This is the number that governs seek cost: it bounds how
/// many frames a `currentTime` seek must decode before it can present.
const BACKDROP_KEYINT: &str = "15";

/// How many trailing fetch-hook output lines to retain for failure messages.
const FETCH_TAIL_LINES: usize = 20;

/// Marker the score sidecar puts on its one-line OMR confidence summary (M13-B).
///
/// A scan import is a lossy inference step, so the sidecar reports how much of it
/// it doubts (`omr: imported 412 notes, 37 flagged — review in the editor`). That
/// summary rides the existing [`Progress::Log`] stream rather than a new channel,
/// and this prefix is how a frontend picks it out of the surrounding engine
/// chatter to put on its import status line. Mirrored in Python as
/// `score_import.confidence.SUMMARY_PREFIX` — change both or neither.
pub const OMR_SUMMARY_PREFIX: &str = "omr: ";

/// The OMR summary carried by a [`Progress::Log`] line, or `None` for any other
/// line. Frontends call this on every log line and keep the last match.
pub fn omr_summary(line: &str) -> Option<&str> {
    line.trim().strip_prefix(OMR_SUMMARY_PREFIX).map(str::trim)
}

/// Input to the import pipeline.
pub enum ImportInput {
    /// A local video file that already exists on disk.
    File(PathBuf),
    /// A URL to download via the configured fetch hook.
    Url(String),
    /// A local score file (MusicXML and friends) — or a scan/PDF — that already
    /// exists on disk. Handled by the score sidecar with no fetch, no ffmpeg and
    /// no retained video either way.
    ///
    /// One variant covers both deliberately: a score file converts exactly
    /// (M13-A), while a scan is transcribed by an OMR engine first and comes back
    /// with per-note confidence (M13-B) — and **the sidecar decides which**, from
    /// the file extension. Nothing on this side has to know, which is what keeps
    /// the whole OMR tier out of the Rust build.
    Score(PathBuf),
}

/// Coarse progress events emitted by [`import_source`].
pub enum Progress {
    /// Downloading via the fetch hook.
    Fetching,
    /// A single line of output from a child process (e.g. the fetch hook).
    /// Frontends render these in a log pane instead of letting the child
    /// write directly to the terminal and scramble the screen.
    Log(String),
    /// Running the Python sidecar (`0.0` = started, `1.0` = complete).
    Extracting(f32),
    /// Writing the chart bundle to disk.
    Writing,
    /// Pipeline completed; the bundle directory path is attached.
    Done(PathBuf),
}

/// Returns `true` when a fetch command is configured for URL imports.
///
/// Checks `ROCKCRAFT_FETCH_CMD` env var first, then `scripts/local/fetch.sh`
/// relative to the workspace root. Used by the TUI to decide whether to show
/// the "Import from URL…" menu item.
pub fn fetch_command_configured() -> bool {
    env_fetch_cmd(&workspace_root()).is_some()
}

/// Resolve → extract → write. `on_progress` lets the TUI render status.
///
/// Handles every [`ImportInput`]: a local video, a URL to fetch, or a local
/// score file. Fetch hook (URL inputs only): resolves `ROCKCRAFT_FETCH_CMD`
/// first, then `scripts/local/fetch.sh` relative to the workspace root. If
/// neither is present, returns [`ImportError::NoFetchCommand`].
pub fn import_source(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ImportError> {
    let workspace = workspace_root();
    let fetch_cmd = env_fetch_cmd(&workspace);
    let ctx = PipelineCtx {
        workspace,
        fetch_cmd,
        ffmpeg_cmd: env_ffmpeg_cmd(),
    };
    run_pipeline(input, on_progress, &ctx)
}

// ── Internal ──────────────────────────────────────────────────────────────────

struct PipelineCtx {
    workspace: PathBuf,
    fetch_cmd: Option<PathBuf>,
    /// Command used to extract the source audio (`ffmpeg` by default, overridable
    /// with `ROCKCRAFT_FFMPEG`). Treated as an optional system dependency: if it
    /// is absent or fails, the import still succeeds with no backing track.
    ffmpeg_cmd: PathBuf,
}

fn run_pipeline(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
    ctx: &PipelineCtx,
) -> Result<PathBuf, ImportError> {
    // A score file is a deterministic transform, not a media pipeline: no fetch,
    // no ffmpeg, no retained movie and no overlay calibration to derive.
    if let ImportInput::Score(path) = input {
        return run_score_pipeline(&path, on_progress, ctx);
    }
    let video_path = resolve_input(input, on_progress, ctx)?;
    let chart_json = run_sidecar(&video_path, on_progress, &ctx.workspace)?;
    let chart = from_json(&chart_json)?;
    on_progress(Progress::Writing);
    let out_dir = ctx
        .workspace
        .join("import-out")
        .join(slug_stamp(&video_path));
    std::fs::create_dir_all(&out_dir)?;
    // Best-effort: attach the source video's audio as the default backing track.
    // ffmpeg is optional — a missing or failing binary leaves `backing: null`.
    let backing = extract_backing(&video_path, &out_dir, &ctx.ffmpeg_cmd);
    // Retain the original source video inside the bundle so the imported piece
    // comes with its background backdrop already attached (M9-G). Best-effort:
    // a copy failure leaves `video: null` and the import still succeeds.
    let video = retain_source_video(&video_path, &out_dir, &ctx.ffmpeg_cmd);
    let bundle = write_chart_bundle_full(&chart, &out_dir, backing, video)?;
    on_progress(Progress::Done(bundle.clone()));
    Ok(bundle)
}

/// Convert a digital score file into a chart bundle (M13-A).
///
/// The score sidecar emits the same [`crate::ExtractedChart`] JSON the video
/// extractor does — including a `notation` block the writer turns into the
/// bundle's grid and key — so the parse/write half is shared verbatim. The
/// bundle is MIDI-only: a score has no audio and no movie, so there is nothing
/// to extract, retain or align against.
fn run_score_pipeline(
    score_path: &Path,
    on_progress: &mut dyn FnMut(Progress),
    ctx: &PipelineCtx,
) -> Result<PathBuf, ImportError> {
    if !score_path.exists() {
        return Err(ImportError::Io(format!(
            "file not found: {}",
            score_path.display()
        )));
    }
    let chart_json = run_score_sidecar(score_path, on_progress, &ctx.workspace)?;
    let chart = from_json(&chart_json)?;
    on_progress(Progress::Writing);
    let out_dir = ctx
        .workspace
        .join("import-out")
        .join(slug_stamp(score_path));
    std::fs::create_dir_all(&out_dir)?;
    let bundle = write_chart_bundle_full(&chart, &out_dir, None, None)?;
    on_progress(Progress::Done(bundle.clone()));
    Ok(bundle)
}

/// Derive an audio file from `video_path` into `out_dir/backing.wav` via ffmpeg
/// (`ffmpeg -i <video> -vn <out>`), returning the [`BackingTrack`] to record in
/// `meta.json`. Returns `None` — leaving the bundle MIDI-only — when ffmpeg is
/// not installed or the extraction fails (e.g. the source has no audio track),
/// so import degrades gracefully on machines without ffmpeg.
fn extract_backing(video_path: &Path, out_dir: &Path, ffmpeg_cmd: &Path) -> Option<BackingTrack> {
    let out_file = out_dir.join(BACKING_FILENAME);
    let status = Command::new(ffmpeg_cmd)
        .arg("-y") // overwrite without prompting
        .arg("-i")
        .arg(video_path)
        .arg("-vn") // drop the video stream; keep audio only
        .arg(&out_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;

    // ffmpeg can exit 0 yet write nothing (e.g. a silent/absent audio stream);
    // require a non-empty file before trusting the track.
    let wrote_audio = status.success()
        && std::fs::metadata(&out_file)
            .map(|m| m.len() > 0)
            .unwrap_or(false);

    if wrote_audio {
        Some(BackingTrack {
            file: BACKING_FILENAME.to_string(),
            audio_start_us: 0,
        })
    } else {
        // Remove any empty/partial file so the bundle stays clean.
        let _ = std::fs::remove_file(&out_file);
        None
    }
}

/// Retain the source video in `out_dir` and return the [`BackgroundVideo`]
/// reference to record in `meta.json` (offset 0 — 1:1 real-time alignment, like
/// M7-tauri-N).
///
/// Prefers a **normalising transcode** (see [`transcode_backdrop`]) so every
/// imported bundle carries a backdrop with the same scrubbing characteristics,
/// falling back to a straight copy when ffmpeg is unavailable or fails.
///
/// Best-effort throughout: returns `None` (leaving the bundle without a
/// backdrop) only when both paths fail, so import degrades gracefully.
fn retain_source_video(
    video_path: &Path,
    out_dir: &Path,
    ffmpeg_cmd: &Path,
) -> Option<BackgroundVideo> {
    if let Some(video) = transcode_backdrop(video_path, out_dir, ffmpeg_cmd) {
        return Some(video);
    }
    copy_source_video(video_path, out_dir)
}

/// Re-encode the source into `out_dir/source.mp4` as a *scrub-friendly* backdrop.
///
/// The editor scrubs the backdrop by setting `currentTime`, and a seek must
/// decode every frame from the preceding keyframe to the target. Whatever a
/// download hands us is tuned for linear playback — typically 60 fps with a ~6 s
/// keyframe interval, i.e. up to ~390 frames of decode *per cursor move*, which
/// in WebView2 leaves the backdrop showing the clip's intro frame for about a
/// second at a time. Normalising to [`BACKDROP_FPS`] with a
/// [`BACKDROP_KEYINT`]-frame GOP cuts that to ~15 frames, and — just as
/// importantly — makes every imported piece behave the same way regardless of
/// what the source happened to be encoded as.
///
/// `-sc_threshold 0` pins the GOP to a fixed cadence rather than letting scene
/// detection place keyframes, so the worst-case seek distance is bounded.
///
/// Returns `None` if ffmpeg is missing or the encode fails/writes nothing; the
/// caller then falls back to copying the original.
fn transcode_backdrop(
    video_path: &Path,
    out_dir: &Path,
    ffmpeg_cmd: &Path,
) -> Option<BackgroundVideo> {
    let filename = format!("{VIDEO_BASENAME}.mp4");
    let dest = out_dir.join(&filename);
    let status = Command::new(ffmpeg_cmd)
        .arg("-y") // overwrite without prompting
        .arg("-i")
        .arg(video_path)
        .args(["-c:v", "libx264"])
        .args(["-preset", "veryfast"])
        .args(["-crf", "23"])
        .args(["-r", BACKDROP_FPS])
        .args(["-g", BACKDROP_KEYINT])
        .args(["-keyint_min", BACKDROP_KEYINT])
        .args(["-sc_threshold", "0"])
        .args(["-pix_fmt", "yuv420p"]) // widest decoder support
        .arg("-an") // audio rides in backing.wav, not the backdrop
        .arg(&dest)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .ok()?;

    // ffmpeg can exit 0 having written nothing usable; require real bytes.
    let wrote = status.success()
        && std::fs::metadata(&dest)
            .map(|m| m.len() > 0)
            .unwrap_or(false);
    if wrote {
        Some(BackgroundVideo {
            file: filename,
            offset_us: 0,
        })
    } else {
        let _ = std::fs::remove_file(&dest); // never leave a partial encode
        None
    }
}

/// Copy the source video into `out_dir/source.<ext>` verbatim — the fallback
/// when [`transcode_backdrop`] cannot run. The bundle-relative filename
/// preserves the source extension so the webview's `<video>` can pick the right
/// decoder.
fn copy_source_video(video_path: &Path, out_dir: &Path) -> Option<BackgroundVideo> {
    let ext = video_path
        .extension()
        .and_then(|e| e.to_str())
        .filter(|e| !e.is_empty());
    let filename = match ext {
        Some(e) => format!("{VIDEO_BASENAME}.{e}"),
        None => VIDEO_BASENAME.to_string(),
    };
    let dest = out_dir.join(&filename);
    match std::fs::copy(video_path, &dest) {
        Ok(len) if len > 0 => Some(BackgroundVideo {
            file: filename,
            offset_us: 0,
        }),
        _ => {
            // Clean up any empty/partial file so the bundle stays tidy.
            let _ = std::fs::remove_file(&dest);
            None
        }
    }
}

fn resolve_input(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
    ctx: &PipelineCtx,
) -> Result<PathBuf, ImportError> {
    match input {
        // `Score` never reaches here — `run_pipeline` intercepts it — but
        // treating it as the local file it is keeps the match total without an
        // `unreachable!`.
        ImportInput::File(p) | ImportInput::Score(p) => {
            if !p.exists() {
                return Err(ImportError::Io(format!("file not found: {}", p.display())));
            }
            Ok(p)
        }
        ImportInput::Url(url) => {
            on_progress(Progress::Fetching);
            let fetch_cmd = ctx.fetch_cmd.as_ref().ok_or(ImportError::NoFetchCommand)?;
            let cache_dir = ctx.workspace.join("import-cache");
            std::fs::create_dir_all(&cache_dir)?;
            let target = cache_dir.join(url_filename(&url));
            run_fetch(fetch_cmd, &url, &target, on_progress)?;
            Ok(target)
        }
    }
}

/// Spawn the fetch hook with piped stdio, forwarding every output line through
/// `on_progress` as a [`Progress::Log`] so the child never writes to the
/// terminal directly. The last [`FETCH_TAIL_LINES`] lines are captured and
/// appended to the error on failure (the child's own message would otherwise
/// be lost).
fn run_fetch(
    fetch_cmd: &Path,
    url: &str,
    target: &Path,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<(), ImportError> {
    let mut child = Command::new(fetch_cmd)
        .arg(url)
        .arg(target)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| {
            ImportError::Io(format!(
                "fetch command `{}` failed to start: {e}",
                fetch_cmd.display()
            ))
        })?;

    // Read stdout and stderr on their own threads and merge their lines onto a
    // single channel; the parent forwards each line and keeps a bounded tail.
    let (tx, rx) = mpsc::channel::<String>();
    let mut readers = Vec::new();
    for pipe in [
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
    ]
    .into_iter()
    .flatten()
    {
        let tx = tx.clone();
        readers.push(thread::spawn(move || {
            for line in BufReader::new(pipe).lines().map_while(Result::ok) {
                if tx.send(line).is_err() {
                    break;
                }
            }
        }));
    }
    drop(tx); // Close the channel once every reader thread has finished.

    let mut tail: VecDeque<String> = VecDeque::with_capacity(FETCH_TAIL_LINES);
    for line in rx {
        if tail.len() == FETCH_TAIL_LINES {
            tail.pop_front();
        }
        tail.push_back(line.clone());
        on_progress(Progress::Log(line));
    }
    for reader in readers {
        let _ = reader.join();
    }

    let status = child.wait().map_err(|e| {
        ImportError::Io(format!(
            "fetch command `{}` failed while running: {e}",
            fetch_cmd.display()
        ))
    })?;
    if !status.success() {
        let mut msg = format!(
            "fetch command `{}` exited with {status}",
            fetch_cmd.display()
        );
        if !tail.is_empty() {
            msg.push_str("\n--- fetch output (tail) ---\n");
            msg.push_str(&tail.into_iter().collect::<Vec<_>>().join("\n"));
        }
        return Err(ImportError::Io(msg));
    }
    Ok(())
}

fn run_sidecar(
    video_path: &Path,
    on_progress: &mut dyn FnMut(Progress),
    workspace: &Path,
) -> Result<String, ImportError> {
    on_progress(Progress::Extracting(0.0));
    let sidecar = find_sidecar(workspace)?;
    let output = Command::new("python3")
        .arg(&sidecar)
        .arg("--in")
        .arg(video_path)
        .arg("--out")
        .arg("-")
        .output()
        .map_err(|e| {
            ImportError::SidecarMissing(format!(
                "could not launch python3: {e}; install python3 and set up \
                 tools/synthesia-extract/ (see docs/IMPORT.md)"
            ))
        })?;
    if !output.status.success() {
        return Err(ImportError::SidecarFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    on_progress(Progress::Extracting(1.0));
    String::from_utf8(output.stdout)
        .map_err(|e| ImportError::SidecarFailed(format!("sidecar output is not valid UTF-8: {e}")))
}

fn find_sidecar(workspace: &Path) -> Result<PathBuf, ImportError> {
    let script = workspace.join("tools/synthesia-extract/extract.py");
    if script.exists() {
        Ok(script)
    } else {
        Err(ImportError::SidecarMissing(format!(
            "sidecar not found at {}; ensure tools/synthesia-extract/ is present \
             and the venv is installed — run `pip install -r \
             tools/synthesia-extract/requirements.txt` in a venv (see docs/IMPORT.md)",
            script.display()
        )))
    }
}

/// Run the M13-A/M13-B score converter over `score_path`, returning its chart JSON.
///
/// Same subprocess contract as [`run_sidecar`]: stdout is the JSON, stderr is
/// diagnostics. Unlike the video path, the diagnostics are forwarded on **success**
/// too, as [`Progress::Log`] events: a scan import (M13-B) reports how many of its
/// notes it doubts there, and a summary nobody sees is no review affordance at all.
/// The sidecar decides which input needs OMR, so this side stays one contract.
fn run_score_sidecar(
    score_path: &Path,
    on_progress: &mut dyn FnMut(Progress),
    workspace: &Path,
) -> Result<String, ImportError> {
    on_progress(Progress::Extracting(0.0));
    let sidecar = find_score_sidecar(workspace)?;
    let output = Command::new("python3")
        .arg(&sidecar)
        .arg("--in")
        .arg(score_path)
        .arg("--out")
        .arg("-")
        .output()
        .map_err(|e| {
            ImportError::SidecarMissing(format!(
                "could not launch python3: {e}; install python3 and set up \
                 tools/score-import/ (see docs/IMPORT.md)"
            ))
        })?;
    if !output.status.success() {
        return Err(ImportError::SidecarFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    for line in String::from_utf8_lossy(&output.stderr).lines() {
        if !line.trim().is_empty() {
            on_progress(Progress::Log(line.to_string()));
        }
    }
    on_progress(Progress::Extracting(1.0));
    String::from_utf8(output.stdout)
        .map_err(|e| ImportError::SidecarFailed(format!("sidecar output is not valid UTF-8: {e}")))
}

fn find_score_sidecar(workspace: &Path) -> Result<PathBuf, ImportError> {
    let script = workspace.join("tools/score-import/convert.py");
    if script.exists() {
        Ok(script)
    } else {
        Err(ImportError::SidecarMissing(format!(
            "score sidecar not found at {}; ensure tools/score-import/ is present \
             and the venv is installed — run `pip install -r \
             tools/score-import/requirements.txt` in a venv (see docs/IMPORT.md)",
            script.display()
        )))
    }
}

fn env_fetch_cmd(workspace: &Path) -> Option<PathBuf> {
    std::env::var("ROCKCRAFT_FETCH_CMD")
        .ok()
        .map(PathBuf::from)
        .or_else(|| {
            let local = workspace.join("scripts/local/fetch.sh");
            if local.exists() {
                Some(local)
            } else {
                None
            }
        })
}

/// Resolve the ffmpeg command: `$ROCKCRAFT_FFMPEG` if set, else bare `ffmpeg`
/// (found via `PATH`). Used to extract the source video's audio (issue #152).
fn env_ffmpeg_cmd() -> PathBuf {
    std::env::var_os("ROCKCRAFT_FFMPEG")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("ffmpeg"))
}

fn workspace_root() -> PathBuf {
    if let Ok(root) = std::env::var("ROCKCRAFT_WORKSPACE") {
        return PathBuf::from(root);
    }
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .parent()
        .and_then(|p| p.parent())
        .unwrap_or(crate_dir)
        .to_path_buf()
}

fn slug_stamp(path: &Path) -> String {
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("import");
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("{safe}-{ts}")
}

/// Derive a stable, collision-free cache filename from a URL.
///
/// The previous implementation used only the URL path's last segment, so every
/// `…/watch?v=…` YouTube URL collapsed to `watch`. We now combine a sanitized
/// tail (path segment, query stripped) with a short hash of the *full* URL, so
/// distinct URLs map to distinct paths while the same URL stays stable across
/// runs (preserving cache reuse).
fn url_filename(url: &str) -> String {
    let tail = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("video");
    let mut safe: String = tail
        .chars()
        .map(|c| {
            if c.is_alphanumeric() || c == '-' || c == '_' || c == '.' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if safe.is_empty() {
        safe.push_str("video");
    }
    format!("{safe}-{}", short_hash(url))
}

/// A short, deterministic hex digest of `s` for disambiguating cache names.
fn short_hash(s: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut hasher);
    format!("{:08x}", hasher.finish() as u32)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{parser::from_json as parse_json, writer::write_chart_bundle};
    use rockcraft_core::RecordingMeta;
    use tempfile::TempDir;

    const FIXTURE_JSON: &str = include_str!("../tests/fixtures/synthetic_chart.json");

    fn ctx_no_fetch(tmp: &TempDir) -> PipelineCtx {
        PipelineCtx {
            workspace: tmp.path().to_path_buf(),
            fetch_cmd: None,
            // Point at a binary that does not exist so audio extraction degrades
            // gracefully without requiring ffmpeg on the test machine.
            ffmpeg_cmd: PathBuf::from("rockcraft-no-such-ffmpeg"),
        }
    }

    /// Parse+write exercised directly against the M6-A fixture — no sidecar needed.
    #[test]
    fn parse_and_write_from_fixture() {
        let chart = parse_json(FIXTURE_JSON).unwrap();
        let tmp = TempDir::new().unwrap();
        let bundle = write_chart_bundle(&chart, tmp.path()).unwrap();
        assert!(bundle.join("song.mid").exists());
        assert!(bundle.join("meta.json").exists());
    }

    /// A URL with no fetch command configured must return `NoFetchCommand`.
    #[test]
    fn url_no_fetch_cmd_returns_error() {
        let tmp = TempDir::new().unwrap();
        let ctx = ctx_no_fetch(&tmp);
        let result = run_pipeline(
            ImportInput::Url("https://example.com/video.mp4".into()),
            &mut |_| {},
            &ctx,
        );
        assert!(matches!(result, Err(ImportError::NoFetchCommand)));
    }

    /// A File input pointing to a workspace with no sidecar must return `SidecarMissing`.
    #[test]
    fn missing_sidecar_returns_error() {
        let tmp = TempDir::new().unwrap();
        let video = tmp.path().join("video.mp4");
        std::fs::write(&video, b"stub").unwrap();
        let ctx = ctx_no_fetch(&tmp);
        let result = run_pipeline(ImportInput::File(video), &mut |_| {}, &ctx);
        assert!(
            matches!(result, Err(ImportError::SidecarMissing(_))),
            "expected SidecarMissing, got {result:?}"
        );
    }

    /// `url_filename` must map two different URLs that share a path tail to
    /// distinct cache filenames (the YouTube `…/watch` collision), while a
    /// single URL stays stable across calls.
    #[test]
    fn url_filename_disambiguates_colliding_tails() {
        let a = url_filename("https://www.youtube.com/watch?v=aaaaaaaaaaa");
        let b = url_filename("https://www.youtube.com/watch?v=bbbbbbbbbbb");
        assert_ne!(a, b, "distinct URLs must not collide: {a} == {b}");
        assert!(a.starts_with("watch-"), "sanitized tail preserved: {a}");
        assert_eq!(
            a,
            url_filename("https://www.youtube.com/watch?v=aaaaaaaaaaa"),
            "same URL must be stable across calls"
        );
    }

    /// A fetch hook's output must arrive as `Progress::Log` events rather than
    /// going to the terminal. `echo` is a stable binary (run_fetch passes it the
    /// url and target as args), so this avoids the fresh-exec ETXTBSY race.
    #[cfg(unix)]
    #[test]
    fn fetch_output_forwarded_as_log_events() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("out.mp4");
        let mut logs = Vec::new();
        run_fetch(
            Path::new("echo"),
            "https://example.com/known-line",
            &target,
            &mut |p| {
                if let Progress::Log(line) = p {
                    logs.push(line);
                }
            },
        )
        .expect("echo fetch should succeed");

        assert!(
            logs.iter()
                .any(|l| l.contains("https://example.com/known-line")),
            "echoed line should arrive as a Log event: {logs:?}"
        );
    }

    /// A failing fetch hook: the error must include the child's own stderr tail.
    #[cfg(unix)]
    #[test]
    fn fetch_failure_error_contains_child_output() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let fetch_sh = tmp.path().join("fetch.sh");
        std::fs::write(
            &fetch_sh,
            "#!/bin/sh\necho 'ERROR: yt-dlp boom' 1>&2\nexit 3\n",
        )
        .unwrap();
        std::fs::set_permissions(&fetch_sh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let target = tmp.path().join("out.mp4");
        let err = run_fetch(&fetch_sh, "https://example.com/v.mp4", &target, &mut |_| {})
            .expect_err("non-zero exit must be an error");
        let msg = err.to_string();
        assert!(
            msg.contains("ERROR: yt-dlp boom"),
            "error should carry the child's stderr tail: {msg}"
        );
    }

    /// Stub fetch + stub sidecar: full pipeline writes a valid bundle without touching
    /// the network or naming any specific video service.
    #[cfg(unix)]
    #[test]
    fn url_with_stubs_writes_bundle() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();

        // Stub video source (content irrelevant — sidecar is also stubbed).
        let src_video = tmp.path().join("source.mp4");
        std::fs::write(&src_video, b"stub-video").unwrap();

        // Stub fetch script: copies the source file to the target path ($2).
        let fetch_sh = tmp.path().join("fetch.sh");
        std::fs::write(
            &fetch_sh,
            format!("#!/bin/sh\ncp '{}' \"$2\"\n", src_video.display()),
        )
        .unwrap();
        std::fs::set_permissions(&fetch_sh, std::fs::Permissions::from_mode(0o755)).unwrap();

        // Stub sidecar: reads fixture from a temp file and writes it to stdout.
        let fixture_path = tmp.path().join("fixture.json");
        std::fs::write(&fixture_path, FIXTURE_JSON).unwrap();
        let sidecar_dir = tmp.path().join("tools/synthesia-extract");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join("extract.py"),
            format!(
                "import sys\nwith open('{}') as f:\n    sys.stdout.write(f.read())\n",
                fixture_path.display()
            ),
        )
        .unwrap();

        let ctx = PipelineCtx {
            workspace: tmp.path().to_path_buf(),
            fetch_cmd: Some(fetch_sh),
            ffmpeg_cmd: PathBuf::from("rockcraft-no-such-ffmpeg"),
        };
        let result = run_pipeline(
            ImportInput::Url("https://example.com/video.mp4".into()),
            &mut |_| {},
            &ctx,
        );

        let bundle = result.expect("pipeline should succeed with stubs");
        assert!(bundle.join("song.mid").exists(), "song.mid missing");
        assert!(bundle.join("meta.json").exists(), "meta.json missing");
    }

    // ── Backing-audio extraction (issue #152, Task B) ────────────────────────

    /// Parse the `backing` filename from a bundle's `meta.json`, if any.
    fn meta_backing_file(bundle: &Path) -> Option<String> {
        let json = std::fs::read_to_string(bundle.join("meta.json")).unwrap();
        let meta = RecordingMeta::from_json(&json).unwrap();
        meta.backing.map(|b| b.file)
    }

    /// Parse the `video` reference from a bundle's `meta.json`, if any.
    fn meta_video(bundle: &Path) -> Option<rockcraft_core::BackgroundVideo> {
        let json = std::fs::read_to_string(bundle.join("meta.json")).unwrap();
        RecordingMeta::from_json(&json).unwrap().video
    }

    /// Build a stub-sidecar workspace and run a File import end to end, returning
    /// the resulting bundle path. ffmpeg is stubbed out (missing binary) so the
    /// backing path degrades; the source video is `clip.<ext>`.
    fn import_file_with_stub_sidecar(tmp: &TempDir, ext: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;

        let video = tmp.path().join(format!("clip.{ext}"));
        std::fs::write(&video, b"stub-video-bytes").unwrap();

        let fixture_path = tmp.path().join("fixture.json");
        std::fs::write(&fixture_path, FIXTURE_JSON).unwrap();
        let sidecar_dir = tmp.path().join("tools/synthesia-extract");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join("extract.py"),
            format!(
                "import sys\nwith open('{}') as f:\n    sys.stdout.write(f.read())\n",
                fixture_path.display()
            ),
        )
        .unwrap();
        let _ = std::fs::set_permissions(
            sidecar_dir.join("extract.py"),
            std::fs::Permissions::from_mode(0o644),
        );

        let ctx = ctx_no_fetch(tmp);
        run_pipeline(ImportInput::File(video), &mut |_| {}, &ctx)
            .expect("import succeeds with stub sidecar")
    }

    /// A File import retains the original source video in the bundle and records
    /// it as the background video (offset 0), preserving the source extension.
    #[cfg(unix)]
    #[test]
    fn import_retains_source_video() {
        let tmp = TempDir::new().unwrap();
        let bundle = import_file_with_stub_sidecar(&tmp, "mp4");

        let video = meta_video(&bundle).expect("imported bundle must carry a background video");
        assert_eq!(video.file, "source.mp4", "source extension preserved");
        assert_eq!(video.offset_us, 0, "imported video aligns 1:1");
        assert!(
            bundle.join("source.mp4").exists(),
            "source video file must be copied into the bundle"
        );
        assert!(
            std::fs::metadata(bundle.join("source.mp4")).unwrap().len() > 0,
            "retained source video must be non-empty"
        );
    }

    /// The retained source video keeps a `.webm` source extension (the webview
    /// `<video>` decoder relies on it).
    #[cfg(unix)]
    #[test]
    fn import_retains_source_video_extension() {
        let tmp = TempDir::new().unwrap();
        let bundle = import_file_with_stub_sidecar(&tmp, "webm");
        assert_eq!(meta_video(&bundle).unwrap().file, "source.webm");
        assert!(bundle.join("source.webm").exists());
    }

    /// `retain_source_video` returns `None` and leaves no file when the source
    /// path does not exist (graceful degradation: audio-only / no source video).
    #[test]
    fn retain_source_video_missing_is_none() {
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let missing = tmp.path().join("does-not-exist.mp4");
        let no_ffmpeg = Path::new("rockcraft-no-such-ffmpeg");
        assert!(retain_source_video(&missing, &out_dir, no_ffmpeg).is_none());
        assert!(!out_dir.join("source.mp4").exists());
    }

    /// Without ffmpeg the backdrop falls back to a verbatim copy — the bundle
    /// still gets its movie, just un-normalised. Guards the degradation path:
    /// the normalising transcode must never be load-bearing for having a
    /// backdrop at all.
    #[test]
    fn retain_source_video_falls_back_to_copy_without_ffmpeg() {
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let src = tmp.path().join("clip.webm");
        std::fs::write(&src, b"not-really-a-video").unwrap();

        let video =
            retain_source_video(&src, &out_dir, Path::new("rockcraft-no-such-ffmpeg")).unwrap();

        // Copied verbatim, so the source extension (and bytes) survive.
        assert_eq!(video.file, "source.webm");
        assert_eq!(video.offset_us, 0);
        assert_eq!(
            std::fs::read(out_dir.join("source.webm")).unwrap(),
            b"not-really-a-video"
        );
        // The failed transcode must not leave a stray .mp4 behind.
        assert!(!out_dir.join("source.mp4").exists());
    }

    /// A *successful* transcode normalises the backdrop to `source.mp4` whatever
    /// the source container was, and the copy fallback does not also run.
    #[cfg(unix)]
    #[test]
    fn retain_source_video_transcodes_to_normalised_mp4() {
        use std::os::unix::fs::PermissionsExt;
        let tmp = TempDir::new().unwrap();
        let out_dir = tmp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        let src = tmp.path().join("clip.webm");
        std::fs::write(&src, b"stub").unwrap();

        // Stub ffmpeg: writes bytes to the last argument (the destination).
        let fake = tmp.path().join("fake-ffmpeg");
        std::fs::write(
            &fake,
            "#!/bin/sh\nfor a in \"$@\"; do d=\"$a\"; done\nprintf transcoded > \"$d\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();

        let video = retain_source_video(&src, &out_dir, &fake).unwrap();

        assert_eq!(video.file, "source.mp4", "normalised regardless of source");
        assert_eq!(
            std::fs::read(out_dir.join("source.mp4")).unwrap(),
            b"transcoded"
        );
        assert!(
            !out_dir.join("source.webm").exists(),
            "copy fallback must not run once the transcode succeeded"
        );
    }

    /// With no usable ffmpeg, a File import still produces a valid bundle whose
    /// backing is `null` — graceful degradation (acceptance: "without ffmpeg the
    /// import still succeeds, just with backing: null").
    #[cfg(unix)]
    #[test]
    fn import_without_ffmpeg_leaves_backing_null() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let video = tmp.path().join("clip.mp4");
        std::fs::write(&video, b"stub-video").unwrap();

        // Stub sidecar emitting the fixture chart.
        let fixture_path = tmp.path().join("fixture.json");
        std::fs::write(&fixture_path, FIXTURE_JSON).unwrap();
        let sidecar_dir = tmp.path().join("tools/synthesia-extract");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join("extract.py"),
            format!(
                "import sys\nwith open('{}') as f:\n    sys.stdout.write(f.read())\n",
                fixture_path.display()
            ),
        )
        .unwrap();
        let _ = std::fs::set_permissions(
            sidecar_dir.join("extract.py"),
            std::fs::Permissions::from_mode(0o644),
        );

        let ctx = ctx_no_fetch(&tmp); // ffmpeg_cmd points at a missing binary
        let bundle = run_pipeline(ImportInput::File(video), &mut |_| {}, &ctx)
            .expect("import succeeds even without ffmpeg");

        assert!(bundle.join("song.mid").exists());
        assert_eq!(
            meta_backing_file(&bundle),
            None,
            "no ffmpeg → backing must be null"
        );
        assert!(
            !bundle.join(BACKING_FILENAME).exists(),
            "no stray backing file should be left behind"
        );
    }

    /// With a stub "ffmpeg" that writes a (synthetic) audio file, a File import
    /// attaches it as the bundle's backing track (acceptance: a fresh import on a
    /// machine with ffmpeg yields a bundle whose backing is the video's audio).
    #[cfg(unix)]
    #[test]
    fn import_with_ffmpeg_attaches_backing() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let video = tmp.path().join("clip.mp4");
        std::fs::write(&video, b"stub-video").unwrap();

        // Stub sidecar emitting the fixture chart.
        let fixture_path = tmp.path().join("fixture.json");
        std::fs::write(&fixture_path, FIXTURE_JSON).unwrap();
        let sidecar_dir = tmp.path().join("tools/synthesia-extract");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join("extract.py"),
            format!(
                "import sys\nwith open('{}') as f:\n    sys.stdout.write(f.read())\n",
                fixture_path.display()
            ),
        )
        .unwrap();

        // Stub ffmpeg: ignores its flags, writes non-empty bytes to the last arg
        // (the output path), standing in for a real audio extraction.
        let ffmpeg_sh = tmp.path().join("ffmpeg.sh");
        std::fs::write(
            &ffmpeg_sh,
            "#!/bin/sh\n# last argument is the output file\nfor out; do :; done\nprintf 'RIFFfake' > \"$out\"\n",
        )
        .unwrap();
        std::fs::set_permissions(&ffmpeg_sh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let ctx = PipelineCtx {
            workspace: tmp.path().to_path_buf(),
            fetch_cmd: None,
            ffmpeg_cmd: ffmpeg_sh,
        };
        let bundle = run_pipeline(ImportInput::File(video), &mut |_| {}, &ctx)
            .expect("import succeeds with ffmpeg");

        assert_eq!(
            meta_backing_file(&bundle).as_deref(),
            Some(BACKING_FILENAME),
            "ffmpeg present → backing points at the extracted audio"
        );
        let audio = bundle.join(BACKING_FILENAME);
        assert!(audio.exists(), "extracted audio file must be written");
        assert!(
            std::fs::metadata(&audio).unwrap().len() > 0,
            "extracted audio must be non-empty"
        );
    }

    /// A stub "ffmpeg" that exits 0 but writes an empty file (mimicking a source
    /// with no audio stream) must NOT attach a backing track, and must clean up
    /// the empty file.
    #[cfg(unix)]
    #[test]
    fn empty_ffmpeg_output_is_not_attached() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = TempDir::new().unwrap();
        let video = tmp.path().join("clip.mp4");
        let out_dir = tmp.path().join("out");
        std::fs::create_dir_all(&out_dir).unwrap();
        std::fs::write(&video, b"stub").unwrap();

        let ffmpeg_sh = tmp.path().join("ffmpeg.sh");
        std::fs::write(
            &ffmpeg_sh,
            "#!/bin/sh\nfor out; do :; done\n: > \"$out\"\n", // create empty file
        )
        .unwrap();
        std::fs::set_permissions(&ffmpeg_sh, std::fs::Permissions::from_mode(0o755)).unwrap();

        let backing = extract_backing(&video, &out_dir, &ffmpeg_sh);
        assert!(backing.is_none(), "empty audio must not be attached");
        assert!(
            !out_dir.join(BACKING_FILENAME).exists(),
            "empty output file must be cleaned up"
        );
    }

    // ── Score import (M13-A) ─────────────────────────────────────────────────

    /// Chart JSON a stub score sidecar emits: one note plus a notation block.
    const SCORE_JSON: &str = r#"{
      "notes": [{"pitch": 67, "start_us": 0, "dur_us": 500000, "hand": "Right", "confidence": 1.0}],
      "source": {"extractor_version": "score-import-0.1"},
      "notation": {"bpm": 90, "time_sig": {"beats_per_bar": 3, "beat_unit": 4},
                   "key": {"root_pc": 7, "scale": "Major"}}
    }"#;

    /// Install a stub `tools/score-import/convert.py` under `tmp` that echoes
    /// `SCORE_JSON` on stdout, standing in for the music21 converter.
    fn stub_score_sidecar(tmp: &TempDir) {
        let fixture_path = tmp.path().join("score-fixture.json");
        std::fs::write(&fixture_path, SCORE_JSON).unwrap();
        let sidecar_dir = tmp.path().join("tools/score-import");
        std::fs::create_dir_all(&sidecar_dir).unwrap();
        std::fs::write(
            sidecar_dir.join("convert.py"),
            format!(
                "import sys\nwith open('{}') as f:\n    sys.stdout.write(f.read())\n",
                fixture_path.display()
            ),
        )
        .unwrap();
    }

    /// A score import writes a MIDI-only bundle whose `meta.json` carries the
    /// notated grid and key — and no movie, backing or alignment sidecar.
    #[test]
    fn score_import_writes_notated_bundle() {
        let tmp = TempDir::new().unwrap();
        stub_score_sidecar(&tmp);
        let score = tmp.path().join("scale.musicxml");
        std::fs::write(&score, b"<score-partwise/>").unwrap();

        let ctx = ctx_no_fetch(&tmp);
        let bundle = run_pipeline(ImportInput::Score(score), &mut |_| {}, &ctx)
            .expect("score import succeeds with a stub sidecar");

        let json = std::fs::read_to_string(bundle.join("meta.json")).unwrap();
        let meta = RecordingMeta::from_json(&json).unwrap();
        let grid = meta.grid.expect("notated grid must reach meta.json");
        assert_eq!(grid.bpm, 90);
        assert_eq!(grid.time_sig.beats_per_bar, 3);
        assert_eq!(
            meta.key,
            Some(rockcraft_core::Key {
                root_pc: 7,
                scale: rockcraft_core::Scale::Major,
            })
        );
        assert!(meta.backing.is_none(), "a score has no audio");
        assert!(meta.video.is_none(), "a score has no movie");
        assert!(bundle.join("song.mid").exists());
        assert!(
            !bundle.join("alignment.json").exists(),
            "no movie → nothing to calibrate an overlay against"
        );
    }

    /// A score import reports Extracting → Writing → Done, and never Fetching.
    #[test]
    fn score_import_progress_skips_fetching() {
        let tmp = TempDir::new().unwrap();
        stub_score_sidecar(&tmp);
        let score = tmp.path().join("scale.musicxml");
        std::fs::write(&score, b"<score-partwise/>").unwrap();

        let mut stages = Vec::new();
        let ctx = ctx_no_fetch(&tmp);
        run_pipeline(
            ImportInput::Score(score),
            &mut |p| {
                stages.push(match p {
                    Progress::Fetching => "fetching",
                    Progress::Log(_) => "log",
                    Progress::Extracting(_) => "extracting",
                    Progress::Writing => "writing",
                    Progress::Done(_) => "done",
                });
            },
            &ctx,
        )
        .unwrap();

        assert!(!stages.contains(&"fetching"), "score import never fetches");
        assert_eq!(stages.last(), Some(&"done"));
        assert!(stages.contains(&"extracting") && stages.contains(&"writing"));
    }

    /// A workspace with no score sidecar names `tools/score-import/` in the error
    /// — not the video extractor's requirements file.
    #[test]
    fn missing_score_sidecar_returns_actionable_error() {
        let tmp = TempDir::new().unwrap();
        let score = tmp.path().join("scale.musicxml");
        std::fs::write(&score, b"<score-partwise/>").unwrap();
        let ctx = ctx_no_fetch(&tmp);

        let err = run_pipeline(ImportInput::Score(score), &mut |_| {}, &ctx)
            .expect_err("no sidecar must fail");
        match err {
            ImportError::SidecarMissing(msg) => assert!(
                msg.contains("tools/score-import/requirements.txt"),
                "error must point at the score sidecar's deps: {msg}"
            ),
            other => panic!("expected SidecarMissing, got {other:?}"),
        }
    }

    /// A score path that does not exist fails before the sidecar is consulted.
    #[test]
    fn missing_score_file_returns_io_error() {
        let tmp = TempDir::new().unwrap();
        stub_score_sidecar(&tmp);
        let ctx = ctx_no_fetch(&tmp);
        let result = run_pipeline(
            ImportInput::Score(tmp.path().join("nope.musicxml")),
            &mut |_| {},
            &ctx,
        );
        assert!(matches!(result, Err(ImportError::Io(_))), "{result:?}");
    }

    // ── Scan import (M13-B) ──────────────────────────────────────────────────

    /// Install a stub score sidecar that also writes `stderr_text` to stderr,
    /// standing in for the OMR path's confidence report.
    fn stub_score_sidecar_with_stderr(tmp: &TempDir, stderr_text: &str) {
        stub_score_sidecar(tmp);
        let sidecar = tmp.path().join("tools/score-import/convert.py");
        let existing = std::fs::read_to_string(&sidecar).unwrap();
        std::fs::write(
            &sidecar,
            format!("{existing}sys.stderr.write({stderr_text:?})\n"),
        )
        .unwrap();
    }

    /// The sidecar's diagnostics must reach the frontends on a **successful**
    /// import, not only in a failure message: on the OMR path they carry the
    /// flagged-note counts that are the whole review affordance.
    #[test]
    fn score_import_forwards_sidecar_diagnostics_as_log_events() {
        let tmp = TempDir::new().unwrap();
        stub_score_sidecar_with_stderr(
            &tmp,
            "using OMR engine oemer\n\nomr: imported 412 notes, 37 flagged — review in the editor\n",
        );
        let scan = tmp.path().join("page.pdf");
        std::fs::write(&scan, b"%PDF-1.4").unwrap();

        let mut logs = Vec::new();
        let ctx = ctx_no_fetch(&tmp);
        run_pipeline(
            ImportInput::Score(scan),
            &mut |p| {
                if let Progress::Log(line) = p {
                    logs.push(line);
                }
            },
            &ctx,
        )
        .expect("a scan import is the same pipeline as a score import");

        assert!(
            logs.iter().any(|l| l.contains("using OMR engine oemer")),
            "engine chatter belongs in the log pane: {logs:?}"
        );
        let summary = logs
            .iter()
            .find_map(|l| omr_summary(l))
            .expect("the summary line must be recoverable from the log stream");
        assert_eq!(
            summary,
            "imported 412 notes, 37 flagged — review in the editor"
        );
        assert!(
            !logs.iter().any(|l| l.trim().is_empty()),
            "blank stderr lines are not log events: {logs:?}"
        );
    }

    /// A plain score file emits no OMR summary, so nothing claims a confidence
    /// story the exact transform does not have.
    #[test]
    fn a_score_file_import_reports_no_omr_summary() {
        let tmp = TempDir::new().unwrap();
        stub_score_sidecar_with_stderr(&tmp, "dropped 1 grace note(s) (by design)\n");
        let score = tmp.path().join("scale.musicxml");
        std::fs::write(&score, b"<score-partwise/>").unwrap();

        let mut logs = Vec::new();
        let ctx = ctx_no_fetch(&tmp);
        run_pipeline(
            ImportInput::Score(score),
            &mut |p| {
                if let Progress::Log(line) = p {
                    logs.push(line);
                }
            },
            &ctx,
        )
        .unwrap();

        assert!(!logs.is_empty(), "by-design drops are still worth showing");
        assert!(logs.iter().all(|l| omr_summary(l).is_none()), "{logs:?}");
    }

    /// The prefix is a marker, not a substring match: only a line that starts
    /// with it is the summary.
    #[test]
    fn omr_summary_matches_only_the_marked_line() {
        assert_eq!(
            omr_summary("omr: imported 8 notes, 0 flagged"),
            Some("imported 8 notes, 0 flagged")
        );
        assert_eq!(
            omr_summary("  omr: imported 8 notes, 0 flagged  "),
            Some("imported 8 notes, 0 flagged"),
            "the sidecar's line may arrive padded"
        );
        for other in [
            "using OMR engine oemer",
            "suspect measures (2): 12, 13",
            "warning: no tempo mark in the score",
            "",
        ] {
            assert!(omr_summary(other).is_none(), "{other:?} is not the summary");
        }
    }
}
