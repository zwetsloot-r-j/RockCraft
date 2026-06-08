use std::path::{Path, PathBuf};
use std::process::Command;

use crate::{error::ImportError, parser::from_json, writer::write_chart_bundle};

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
            let status = Command::new(fetch_cmd)
                .arg(&url)
                .arg(&target)
                .status()
                .map_err(|e| {
                    ImportError::Io(format!(
                        "fetch command `{}` failed to start: {e}",
                        fetch_cmd.display()
                    ))
                })?;
            if !status.success() {
                return Err(ImportError::Io(format!(
                    "fetch command `{}` exited with {}",
                    fetch_cmd.display(),
                    status
                )));
            }
            Ok(target)
        }
    }
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

fn url_filename(url: &str) -> String {
    url.rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .unwrap_or("video")
        .split('?')
        .next()
        .unwrap_or("video")
        .to_string()
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
