//! Local headless import runner (not part of the shipped UI).
//!
//! The TUI's import flow is an interactive menu and the Tauri `import_start`
//! host command runs on the Windows host where the Python sidecar/ffmpeg/yt-dlp
//! aren't available. This example lets an agent run the *same* pipeline
//! (`import_video`) from WSL against a local file or URL and print progress.
//!
//!   cargo run -p rockcraft-import --example run_import -- <file-or-url>
//!
//! Prints the resulting bundle directory on success.

use std::path::Path;

use rockcraft_import::{import_video, ImportInput, Progress};

fn main() {
    let arg = std::env::args().nth(1).unwrap_or_else(|| {
        eprintln!("usage: run_import <video-file-or-url>");
        std::process::exit(2);
    });

    let input = if arg.starts_with("http://") || arg.starts_with("https://") {
        ImportInput::Url(arg)
    } else {
        ImportInput::File(Path::new(&arg).to_path_buf())
    };

    let mut last_pct = -1i32;
    let result = import_video(input, &mut |p| match p {
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
