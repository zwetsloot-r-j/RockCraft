use std::collections::VecDeque;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;

use crate::{error::ImportError, parser::from_json, writer::write_chart_bundle};

/// How many trailing fetch-hook output lines to retain for failure messages.
const FETCH_TAIL_LINES: usize = 20;

/// Input to the import pipeline.
pub enum ImportInput {
    /// A local video file that already exists on disk.
    File(PathBuf),
    /// A URL to download via the configured fetch hook.
    Url(String),
}

/// Coarse progress events emitted by [`import_video`].
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
/// Fetch hook: resolves `ROCKCRAFT_FETCH_CMD` first, then
/// `scripts/local/fetch.sh` relative to the workspace root.
/// If neither is present, returns [`ImportError::NoFetchCommand`].
pub fn import_video(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
) -> Result<PathBuf, ImportError> {
    let workspace = workspace_root();
    let fetch_cmd = env_fetch_cmd(&workspace);
    let ctx = PipelineCtx {
        workspace,
        fetch_cmd,
    };
    run_pipeline(input, on_progress, &ctx)
}

// ── Internal ──────────────────────────────────────────────────────────────────

struct PipelineCtx {
    workspace: PathBuf,
    fetch_cmd: Option<PathBuf>,
}

fn run_pipeline(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
    ctx: &PipelineCtx,
) -> Result<PathBuf, ImportError> {
    let video_path = resolve_input(input, on_progress, ctx)?;
    let chart_json = run_sidecar(&video_path, on_progress, &ctx.workspace)?;
    let chart = from_json(&chart_json)?;
    on_progress(Progress::Writing);
    let out_dir = ctx
        .workspace
        .join("import-out")
        .join(slug_stamp(&video_path));
    let bundle = write_chart_bundle(&chart, &out_dir)?;
    on_progress(Progress::Done(bundle.clone()));
    Ok(bundle)
}

fn resolve_input(
    input: ImportInput,
    on_progress: &mut dyn FnMut(Progress),
    ctx: &PipelineCtx,
) -> Result<PathBuf, ImportError> {
    match input {
        ImportInput::File(p) => {
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
    use tempfile::TempDir;

    const FIXTURE_JSON: &str = include_str!("../tests/fixtures/synthetic_chart.json");

    fn ctx_no_fetch(tmp: &TempDir) -> PipelineCtx {
        PipelineCtx {
            workspace: tmp.path().to_path_buf(),
            fetch_cmd: None,
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
}
