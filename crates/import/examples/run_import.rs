//! Local headless import runner (not part of the shipped UI).
//!
//! The TUI's import flow is an interactive menu and the Tauri `import_start`
//! host command runs on the Windows host where the Python sidecar/ffmpeg/yt-dlp
//! aren't available. This example lets an agent run the *same* pipeline
//! (`import_source`) from WSL against a local file or URL and print progress.
//!
//!   cargo run -p rockcraft-import --example run_import -- <file-or-url>
//!
//! A score file (`.musicxml`, `.xml`, `.mxl`, `.abc`, `.krn`) routes to the
//! M13-A score converter; any other local path is treated as a video.
//!
//! Prints the resulting bundle directory on success.

use std::path::Path;

use rockcraft_import::{import_source, ImportInput, Progress};

/// Extensions routed to the score sidecar rather than the video extractor.
/// Mirrors the documented/tested set in `tools/score-import/README.md`.
const SCORE_EXTENSIONS: &[&str] = &["musicxml", "xml", "mxl", "abc", "krn"];

fn is_score_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| SCORE_EXTENSIONS.contains(&e.to_lowercase().as_str()))
        .unwrap_or(false)
}

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: run_import <video-file|score-file|url>");
        std::process::exit(2);
    });

    let input = if arg.starts_with("http://") || arg.starts_with("https://") {
        ImportInput::Url(arg)
    } else {
        let path = Path::new(&arg).to_path_buf();
        if is_score_file(&path) {
            ImportInput::Score(path)
        } else {
            ImportInput::File(path)
        }
    };

    let mut last_pct = -1i32;
    let result = import_source(input, &mut |p| match p {
        Progress::Fetching => eprintln!("[fetch] downloading…"),
        Progress::Log(line) => eprintln!("[log] {line}"),
        Progress::Extracting(f) => {
            let pct = (f * 100.0) as i32;
            if pct != last_pct {
                last_pct = pct;
                eprintln!("[extract] {pct}%");
            }
        }
        Progress::Writing => eprintln!("[write] building bundle…"),
        Progress::Done(dir) => eprintln!("[done] {}", dir.display()),
    });

    match result {
        Ok(dir) => println!("{}", dir.display()),
        Err(e) => {
            eprintln!("import failed: {e}");
            std::process::exit(1);
        }
    }
}
