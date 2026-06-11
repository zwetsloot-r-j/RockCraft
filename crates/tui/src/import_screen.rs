//! Import flow: video-file picker → URL input → progress screen.
//!
//! Three self-contained sub-screens that `app.rs` routes through:
//! 1. `VideoPicker`    — browse for a local video file.
//! 2. `UrlInput`       — type a URL.
//! 3. `ImportingScreen`— shows pipeline progress; on completion yields a bundle path.

use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Gauge, List, ListItem, ListState, Paragraph},
    Frame,
};
use rockcraft_import::{import_video, ImportInput, Progress};

// ── Video file picker ──────────────────────────────────────────────────────────

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v"];

/// Return video files in `dir`, sorted by name.
pub fn list_video_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| VIDEO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

#[derive(Debug, Clone, PartialEq)]
pub enum VideoPickerOutcome {
    Selected(PathBuf),
    Cancelled,
    Pending,
}

pub struct VideoPicker {
    dir: PathBuf,
    files: Vec<PathBuf>,
    state: ListState,
    outcome: Option<VideoPickerOutcome>,
}

impl VideoPicker {
    pub fn new(dir: PathBuf) -> Self {
        let files = list_video_files(&dir);
        let mut state = ListState::default();
        if !files.is_empty() {
            state.select(Some(0));
        }
        Self {
            dir,
            files,
            state,
            outcome: None,
        }
    }

    pub fn on_key(&mut self, code: KeyCode) -> &VideoPickerOutcome {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Enter => {
                if let Some(idx) = self.state.selected() {
                    if let Some(path) = self.files.get(idx) {
                        self.outcome = Some(VideoPickerOutcome::Selected(path.clone()));
                    }
                }
                if self.outcome.is_none() {
                    self.outcome = Some(VideoPickerOutcome::Cancelled);
                }
            }
            KeyCode::Esc => {
                self.outcome = Some(VideoPickerOutcome::Cancelled);
            }
            _ => {}
        }
        self.outcome
            .as_ref()
            .unwrap_or(&VideoPickerOutcome::Pending)
    }

    fn move_sel(&mut self, delta: isize) {
        if self.files.is_empty() {
            return;
        }
        let n = self.files.len() as isize;
        let cur = self.state.selected().unwrap_or(0) as isize;
        self.state
            .select(Some((cur + delta).rem_euclid(n) as usize));
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(1),
        ])
        .split(area);

        f.render_widget(
            Paragraph::new(Line::from(format!(
                "Import from video file  (dir: {})  — ↑/↓/j/k move · Enter select · Esc cancel",
                self.dir.display()
            ))),
            chunks[0],
        );

        let items: Vec<ListItem> = if self.files.is_empty() {
            vec![ListItem::new("(no video files found in this directory)")]
        } else {
            self.files
                .iter()
                .map(|p| {
                    ListItem::new(
                        p.file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| p.display().to_string()),
                    )
                })
                .collect()
        };

        let list = List::new(items)
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .title(" video files "),
            )
            .highlight_style(
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");
        let mut state = self.state.clone();
        f.render_stateful_widget(list, chunks[1], &mut state);

        f.render_widget(
            Paragraph::new(Line::from(format!(
                "{} file(s) in directory",
                self.files.len()
            )))
            .style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }
}

// ── URL input ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq)]
pub enum UrlInputOutcome {
    Submitted(String),
    Cancelled,
    Pending,
}

pub struct UrlInput {
    input: String,
    outcome: Option<UrlInputOutcome>,
}

impl Default for UrlInput {
    fn default() -> Self {
        Self::new()
    }
}

impl UrlInput {
    pub fn new() -> Self {
        Self {
            input: String::new(),
            outcome: None,
        }
    }

    pub fn on_key(&mut self, code: KeyCode) -> &UrlInputOutcome {
        match code {
            KeyCode::Char(c) => {
                self.input.push(c);
            }
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Enter => {
                let url = self.input.trim().to_string();
                self.outcome = if url.is_empty() {
                    Some(UrlInputOutcome::Cancelled)
                } else {
                    Some(UrlInputOutcome::Submitted(url))
                };
            }
            KeyCode::Esc => {
                self.outcome = Some(UrlInputOutcome::Cancelled);
            }
            _ => {}
        }
        self.outcome.as_ref().unwrap_or(&UrlInputOutcome::Pending)
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Min(0),
        ])
        .split(area);

        f.render_widget(
            Paragraph::new(Line::from(
                "Import from URL — type the video URL then Enter · Esc cancel",
            )),
            chunks[0],
        );

        f.render_widget(
            Paragraph::new(Line::from(self.input.as_str()))
                .block(Block::default().borders(Borders::ALL).title(" URL ")),
            chunks[1],
        );

        if !self.input.is_empty() {
            f.render_widget(
                Paragraph::new(Line::from(format!("{} chars", self.input.len())))
                    .style(Style::default().fg(Color::DarkGray)),
                chunks[2],
            );
        }
    }
}

// ── Importing progress screen ──────────────────────────────────────────────────

/// Most log lines we retain in memory for the scrolling output pane.
const MAX_LOG_LINES: usize = 500;

enum RunningState {
    Starting,
    Fetching,
    Extracting(f32),
    Writing,
}

enum WorkerEvent {
    Running(RunningState),
    Log(String),
    Done(PathBuf),
    Failed(String),
}

/// Outcome once the import completes (success or failure).
pub enum ImportOutcome {
    Done(PathBuf),
    Failed(String),
}

pub struct ImportingScreen {
    rx: mpsc::Receiver<WorkerEvent>,
    running: RunningState,
    log: VecDeque<String>,
    outcome: Option<ImportOutcome>,
}

impl ImportingScreen {
    /// Spawn the import pipeline on a background thread and return the screen.
    pub fn start(input: ImportInput) -> Self {
        let (tx, rx) = mpsc::channel::<WorkerEvent>();
        thread::spawn(move || {
            let result = import_video(input, &mut |p| {
                let event = match p {
                    Progress::Fetching => WorkerEvent::Running(RunningState::Fetching),
                    Progress::Log(line) => WorkerEvent::Log(line),
                    Progress::Extracting(f) => WorkerEvent::Running(RunningState::Extracting(f)),
                    Progress::Writing => WorkerEvent::Running(RunningState::Writing),
                    Progress::Done(path) => WorkerEvent::Done(path),
                };
                let _ = tx.send(event);
            });
            if let Err(e) = result {
                let _ = tx.send(WorkerEvent::Failed(e.to_string()));
            }
        });
        Self {
            rx,
            running: RunningState::Starting,
            log: VecDeque::new(),
            outcome: None,
        }
    }

    /// Drain the worker channel. Returns a reference to the outcome when finished.
    pub fn poll(&mut self) -> Option<&ImportOutcome> {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::Running(s) => self.running = s,
                WorkerEvent::Log(line) => {
                    if self.log.len() == MAX_LOG_LINES {
                        self.log.pop_front();
                    }
                    self.log.push_back(line);
                }
                WorkerEvent::Done(path) => {
                    self.outcome = Some(ImportOutcome::Done(path));
                }
                WorkerEvent::Failed(msg) => {
                    self.outcome = Some(ImportOutcome::Failed(msg));
                }
            }
        }
        self.outcome.as_ref()
    }

    /// True once the import has failed (the screen lingers to show the tail).
    pub fn has_failed(&self) -> bool {
        matches!(self.outcome, Some(ImportOutcome::Failed(_)))
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Min(0),
        ])
        .split(area);

        let header = match &self.outcome {
            Some(ImportOutcome::Failed(_)) => "Import failed.  (Esc to dismiss)",
            _ => "Importing video — please wait…  (Esc to cancel)",
        };
        f.render_widget(Paragraph::new(Line::from(header)), chunks[0]);

        let (label, ratio): (&str, f64) = match &self.running {
            RunningState::Starting => ("Starting…", 0.0),
            RunningState::Fetching => ("Fetching video…", 0.1),
            RunningState::Extracting(f) => ("Extracting notes…", 0.2 + (*f as f64) * 0.7),
            RunningState::Writing => ("Writing chart bundle…", 0.95),
        };

        let gauge = Gauge::default()
            .block(Block::default().borders(Borders::ALL))
            .gauge_style(Style::default().fg(Color::Cyan))
            .ratio(ratio)
            .label(label);
        f.render_widget(gauge, chunks[1]);

        // Status line: success path, or the failure message (first line).
        match &self.outcome {
            Some(ImportOutcome::Done(path)) => {
                f.render_widget(
                    Paragraph::new(Line::from(format!("Done: {}", path.display())))
                        .style(Style::default().fg(Color::Green)),
                    chunks[2],
                );
            }
            Some(ImportOutcome::Failed(msg)) => {
                let first = msg.lines().next().unwrap_or(msg);
                f.render_widget(
                    Paragraph::new(Line::from(format!("Error: {first}")))
                        .style(Style::default().fg(Color::Red)),
                    chunks[2],
                );
            }
            None => {}
        }

        self.draw_log_pane(f, chunks[3]);
    }

    /// Render the last N captured output lines (N = visible pane height) in a
    /// bordered, scrolling pane. The pane is empty until the fetch hook emits
    /// output and, on failure, keeps the tail visible alongside the status line.
    fn draw_log_pane(&self, f: &mut Frame, area: Rect) {
        if area.height < 3 || self.log.is_empty() {
            return;
        }
        // Two rows are consumed by the top/bottom border.
        let visible = area.height.saturating_sub(2) as usize;
        let start = self.log.len().saturating_sub(visible);
        let lines: Vec<Line> = self
            .log
            .iter()
            .skip(start)
            .map(|l| Line::from(l.as_str()))
            .collect();
        f.render_widget(
            Paragraph::new(lines)
                .block(Block::default().borders(Borders::ALL).title(" output "))
                .style(Style::default().fg(Color::DarkGray)),
            area,
        );
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rockcraft_import_{suffix}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"").unwrap();
    }

    // ── list_video_files ──────────────────────────────────────────────────────

    #[test]
    fn video_list_returns_only_video_extensions() {
        let dir = temp_dir("video_ext");
        touch(&dir, "clip.mp4");
        touch(&dir, "movie.MKV");
        touch(&dir, "audio.mp3");
        touch(&dir, "readme.txt");
        fs::create_dir_all(dir.join("subdir")).unwrap();

        let files = list_video_files(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"clip.mp4".to_string()));
        assert!(names.contains(&"movie.MKV".to_string()));
        assert!(!names.contains(&"audio.mp3".to_string()));
        assert!(!names.contains(&"readme.txt".to_string()));
        assert!(!names.iter().any(|n| n == "subdir"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_list_sorted_alphabetically() {
        let dir = temp_dir("video_sorted");
        touch(&dir, "zzz.mp4");
        touch(&dir, "aaa.mkv");
        touch(&dir, "mmm.avi");

        let files = list_video_files(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["aaa.mkv", "mmm.avi", "zzz.mp4"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_list_empty_on_nonexistent_dir() {
        let files = list_video_files(Path::new("/nonexistent/path/rockcraft_test"));
        assert!(files.is_empty());
    }

    // ── VideoPicker ───────────────────────────────────────────────────────────

    #[test]
    fn video_picker_esc_cancels() {
        let dir = temp_dir("vpick_esc");
        touch(&dir, "clip.mp4");

        let mut picker = VideoPicker::new(dir.clone());
        let outcome = picker.on_key(KeyCode::Esc).clone();
        assert_eq!(outcome, VideoPickerOutcome::Cancelled);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_picker_enter_selects_first() {
        let dir = temp_dir("vpick_enter");
        touch(&dir, "alpha.mp4");
        touch(&dir, "beta.mkv");

        let mut picker = VideoPicker::new(dir.clone());
        let outcome = picker.on_key(KeyCode::Enter).clone();
        match outcome {
            VideoPickerOutcome::Selected(p) => {
                assert_eq!(
                    p.file_name().unwrap().to_string_lossy().as_ref(),
                    "alpha.mp4"
                );
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_picker_navigate_then_enter() {
        let dir = temp_dir("vpick_nav");
        touch(&dir, "alpha.mp4");
        touch(&dir, "beta.mkv");

        let mut picker = VideoPicker::new(dir.clone());
        picker.on_key(KeyCode::Down);
        let outcome = picker.on_key(KeyCode::Enter).clone();
        match outcome {
            VideoPickerOutcome::Selected(p) => {
                assert_eq!(
                    p.file_name().unwrap().to_string_lossy().as_ref(),
                    "beta.mkv"
                );
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_picker_empty_dir_enter_cancels() {
        let dir = temp_dir("vpick_empty");
        let mut picker = VideoPicker::new(dir.clone());
        let outcome = picker.on_key(KeyCode::Enter).clone();
        assert_eq!(outcome, VideoPickerOutcome::Cancelled);
        fs::remove_dir_all(&dir).ok();
    }

    // ── UrlInput ──────────────────────────────────────────────────────────────

    #[test]
    fn url_input_type_and_submit() {
        let mut ui = UrlInput::new();
        for c in "https://example.com/v.mp4".chars() {
            ui.on_key(KeyCode::Char(c));
        }
        let outcome = ui.on_key(KeyCode::Enter).clone();
        assert_eq!(
            outcome,
            UrlInputOutcome::Submitted("https://example.com/v.mp4".into())
        );
    }

    #[test]
    fn url_input_empty_enter_cancels() {
        let mut ui = UrlInput::new();
        let outcome = ui.on_key(KeyCode::Enter).clone();
        assert_eq!(outcome, UrlInputOutcome::Cancelled);
    }

    #[test]
    fn url_input_esc_cancels() {
        let mut ui = UrlInput::new();
        for c in "https://example.com".chars() {
            ui.on_key(KeyCode::Char(c));
        }
        let outcome = ui.on_key(KeyCode::Esc).clone();
        assert_eq!(outcome, UrlInputOutcome::Cancelled);
    }

    #[test]
    fn url_input_backspace_removes_char() {
        let mut ui = UrlInput::new();
        ui.on_key(KeyCode::Char('a'));
        ui.on_key(KeyCode::Char('b'));
        ui.on_key(KeyCode::Backspace);
        let outcome = ui.on_key(KeyCode::Enter).clone();
        assert_eq!(outcome, UrlInputOutcome::Submitted("a".into()));
    }

    // ── ImportingScreen ───────────────────────────────────────────────────────

    #[test]
    fn importing_screen_missing_file_fails() {
        let mut screen = ImportingScreen::start(ImportInput::File(PathBuf::from(
            "/nonexistent/path/video.mp4",
        )));
        // Give the worker thread time to finish.
        std::thread::sleep(std::time::Duration::from_millis(200));
        let outcome = screen.poll();
        assert!(
            matches!(outcome, Some(ImportOutcome::Failed(_))),
            "expected Failed for missing file"
        );
    }

    #[test]
    fn importing_screen_lingers_on_failure() {
        // A failed import must report `has_failed()` so the screen can linger
        // and keep its output tail visible instead of bouncing to the menu.
        let mut screen = ImportingScreen::start(ImportInput::File(PathBuf::from(
            "/nonexistent/path/video.mp4",
        )));
        std::thread::sleep(std::time::Duration::from_millis(200));
        let _ = screen.poll();
        assert!(
            screen.has_failed(),
            "failed import should report has_failed"
        );
    }

    #[test]
    fn importing_screen_no_fetch_cmd_url_fails() {
        // Without ROCKCRAFT_FETCH_CMD set and no local fetch.sh, a URL import
        // must surface a clear error.
        // Remove env var for isolation.
        let prev = std::env::var("ROCKCRAFT_FETCH_CMD").ok();
        unsafe {
            std::env::remove_var("ROCKCRAFT_FETCH_CMD");
        }

        let mut screen =
            ImportingScreen::start(ImportInput::Url("https://example.com/video.mp4".into()));
        std::thread::sleep(std::time::Duration::from_millis(200));
        let outcome = screen.poll();
        assert!(
            matches!(outcome, Some(ImportOutcome::Failed(_))),
            "expected Failed when no fetch command is available"
        );

        // Restore env.
        if let Some(v) = prev {
            unsafe {
                std::env::set_var("ROCKCRAFT_FETCH_CMD", v);
            }
        }
    }
}
