//! Import flow: source-file picker → URL input → progress screen.
//!
//! Three self-contained sub-screens that `app.rs` routes through:
//! 1. `SourcePicker`   — browse for a local video or score file.
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
use rockcraft_import::{import_source, omr_summary, ImportInput, Progress};

// ── Source file picker ─────────────────────────────────────────────────────────

const VIDEO_EXTENSIONS: &[&str] = &["mp4", "mkv", "avi", "mov", "webm", "flv", "wmv", "m4v"];

/// Inputs the score sidecar reads. Mirrors the documented/tested set in
/// `tools/score-import/README.md`: score files, which convert exactly (M13-A),
/// plus scans and PDFs, which the sidecar transcribes with an OMR engine first
/// and which therefore need reviewing (M13-B).
///
/// Both kinds enter through [`SourceKind::Score`] on purpose — the sidecar
/// decides which is which, so the picker never has to.
const SCORE_EXTENSIONS: &[&str] = &[
    "musicxml", "xml", "mxl", "abc", "krn", // score files
    "pdf", "png", "jpg", "jpeg", "tif", "tiff", "bmp", // scans
];

/// Which import source a [`SourcePicker`] browses for.
///
/// The two paths differ in more than the file filter: video goes through
/// fetch/extract/ffmpeg, a score is a deterministic conversion. The kind travels
/// with the picker so the selected path becomes the right [`ImportInput`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceKind {
    Video,
    Score,
}

impl SourceKind {
    fn extensions(self) -> &'static [&'static str] {
        match self {
            SourceKind::Video => VIDEO_EXTENSIONS,
            SourceKind::Score => SCORE_EXTENSIONS,
        }
    }

    fn noun(self) -> &'static str {
        match self {
            SourceKind::Video => "video",
            // Both kinds sit under one picker, so the label has to name both —
            // otherwise the `.pdf`s in the list look like a bug.
            SourceKind::Score => "score or scan",
        }
    }

    /// The pipeline input for a file the user picked under this kind.
    pub fn input_for(self, path: PathBuf) -> ImportInput {
        match self {
            SourceKind::Video => ImportInput::File(path),
            SourceKind::Score => ImportInput::Score(path),
        }
    }
}

/// Return the files in `dir` matching `kind`, sorted by name.
pub fn list_source_files(dir: &Path, kind: SourceKind) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return vec![];
    };
    let extensions = kind.extensions();
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .map(|e| extensions.contains(&e.to_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourcePickerOutcome {
    Selected(PathBuf),
    Cancelled,
    Pending,
}

pub struct SourcePicker {
    dir: PathBuf,
    kind: SourceKind,
    files: Vec<PathBuf>,
    state: ListState,
    outcome: Option<SourcePickerOutcome>,
}

impl SourcePicker {
    pub fn new(dir: PathBuf, kind: SourceKind) -> Self {
        let files = list_source_files(&dir, kind);
        let mut state = ListState::default();
        if !files.is_empty() {
            state.select(Some(0));
        }
        Self {
            dir,
            kind,
            files,
            state,
            outcome: None,
        }
    }

    pub fn kind(&self) -> SourceKind {
        self.kind
    }

    pub fn on_key(&mut self, code: KeyCode) -> &SourcePickerOutcome {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Enter => {
                if let Some(idx) = self.state.selected() {
                    if let Some(path) = self.files.get(idx) {
                        self.outcome = Some(SourcePickerOutcome::Selected(path.clone()));
                    }
                }
                if self.outcome.is_none() {
                    self.outcome = Some(SourcePickerOutcome::Cancelled);
                }
            }
            KeyCode::Esc => {
                self.outcome = Some(SourcePickerOutcome::Cancelled);
            }
            _ => {}
        }
        self.outcome
            .as_ref()
            .unwrap_or(&SourcePickerOutcome::Pending)
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
                "Import from {} file  (dir: {})  — ↑/↓/j/k move · Enter select · Esc cancel",
                self.kind.noun(),
                self.dir.display()
            ))),
            chunks[0],
        );

        let items: Vec<ListItem> = if self.files.is_empty() {
            vec![ListItem::new(format!(
                "(no {} files found in this directory)",
                self.kind.noun()
            ))]
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
                    .title(format!(" {} files ", self.kind.noun())),
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

pub(crate) enum RunningState {
    Starting,
    Fetching,
    Extracting(f32),
    Writing,
}

pub(crate) enum WorkerEvent {
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
    /// The OMR confidence summary lifted out of the log stream (M13-B), shown on
    /// its own status line. `None` for every import that has nothing to doubt.
    summary: Option<String>,
    outcome: Option<ImportOutcome>,
}

impl ImportingScreen {
    /// Spawn the import pipeline on a background thread and return the screen.
    pub fn start(input: ImportInput) -> Self {
        let (tx, rx) = mpsc::channel::<WorkerEvent>();
        thread::spawn(move || {
            let result = import_source(input, &mut |p| {
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
            summary: None,
            outcome: None,
        }
    }

    /// A screen fed by `rx` instead of a live pipeline, so the log/summary
    /// handling — and the shell's completion branch — are testable without a
    /// Python sidecar on the machine.
    #[cfg(test)]
    pub(crate) fn from_receiver(rx: mpsc::Receiver<WorkerEvent>) -> Self {
        Self {
            rx,
            running: RunningState::Starting,
            log: VecDeque::new(),
            summary: None,
            outcome: None,
        }
    }

    /// Drain the worker channel. Returns a reference to the outcome when finished.
    pub fn poll(&mut self) -> Option<&ImportOutcome> {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                WorkerEvent::Running(s) => self.running = s,
                WorkerEvent::Log(line) => {
                    // A scan import's confidence summary rides the same log
                    // stream as the engine's chatter; promote it out of the pane
                    // so the counts are readable without scrolling.
                    if let Some(summary) = omr_summary(&line) {
                        self.summary = Some(summary.to_string());
                    }
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

    /// The OMR confidence summary, once the sidecar has reported one.
    pub fn summary(&self) -> Option<&str> {
        self.summary.as_deref()
    }

    pub fn draw(&self, f: &mut Frame, area: Rect) {
        let chunks = Layout::vertical([
            Constraint::Length(2),
            Constraint::Length(3),
            Constraint::Length(1),
            Constraint::Length(if self.summary.is_some() { 1 } else { 0 }),
            Constraint::Min(0),
        ])
        .split(area);

        let header = match &self.outcome {
            Some(ImportOutcome::Failed(_)) => "Import failed.  (Esc to dismiss)",
            _ => "Importing — please wait…  (Esc to cancel)",
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

        // A scan import's own status line: how much of the chart it doubts. Shown
        // as soon as the sidecar reports it, so it is on screen before the
        // "Done:" line and stays there afterwards.
        if let Some(summary) = &self.summary {
            f.render_widget(
                Paragraph::new(Line::from(summary.as_str()))
                    .style(Style::default().fg(Color::Yellow)),
                chunks[3],
            );
        }

        self.draw_log_pane(f, chunks[4]);
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

    // ── list_source_files ─────────────────────────────────────────────────────

    #[test]
    fn video_list_returns_only_video_extensions() {
        let dir = temp_dir("video_ext");
        touch(&dir, "clip.mp4");
        touch(&dir, "movie.MKV");
        touch(&dir, "audio.mp3");
        touch(&dir, "readme.txt");
        fs::create_dir_all(dir.join("subdir")).unwrap();

        let files = list_source_files(&dir, SourceKind::Video);
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

        let files = list_source_files(&dir, SourceKind::Video);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["aaa.mkv", "mmm.avi", "zzz.mp4"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_list_empty_on_nonexistent_dir() {
        let files = list_source_files(
            Path::new("/nonexistent/path/rockcraft_test"),
            SourceKind::Video,
        );
        assert!(files.is_empty());
    }

    // ── SourcePicker ──────────────────────────────────────────────────────────

    #[test]
    fn video_picker_esc_cancels() {
        let dir = temp_dir("vpick_esc");
        touch(&dir, "clip.mp4");

        let mut picker = SourcePicker::new(dir.clone(), SourceKind::Video);
        let outcome = picker.on_key(KeyCode::Esc).clone();
        assert_eq!(outcome, SourcePickerOutcome::Cancelled);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn video_picker_enter_selects_first() {
        let dir = temp_dir("vpick_enter");
        touch(&dir, "alpha.mp4");
        touch(&dir, "beta.mkv");

        let mut picker = SourcePicker::new(dir.clone(), SourceKind::Video);
        let outcome = picker.on_key(KeyCode::Enter).clone();
        match outcome {
            SourcePickerOutcome::Selected(p) => {
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

        let mut picker = SourcePicker::new(dir.clone(), SourceKind::Video);
        picker.on_key(KeyCode::Down);
        let outcome = picker.on_key(KeyCode::Enter).clone();
        match outcome {
            SourcePickerOutcome::Selected(p) => {
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
        let mut picker = SourcePicker::new(dir.clone(), SourceKind::Video);
        let outcome = picker.on_key(KeyCode::Enter).clone();
        assert_eq!(outcome, SourcePickerOutcome::Cancelled);
        fs::remove_dir_all(&dir).ok();
    }

    // ── Score picker (M13-A) ──────────────────────────────────────────────────

    /// The score picker lists score formats and hides the videos sitting next to
    /// them (and vice-versa — the two kinds must not bleed into each other).
    #[test]
    fn score_list_returns_only_score_extensions() {
        let dir = temp_dir("score_ext");
        touch(&dir, "scale.musicxml");
        touch(&dir, "tune.MXL");
        touch(&dir, "chorale.krn");
        touch(&dir, "clip.mp4");
        touch(&dir, "readme.txt");

        let names: Vec<String> = list_source_files(&dir, SourceKind::Score)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"scale.musicxml".to_string()));
        assert!(names.contains(&"tune.MXL".to_string()), "case-insensitive");
        assert!(names.contains(&"chorale.krn".to_string()));
        assert!(!names.contains(&"clip.mp4".to_string()));
        assert!(!names.contains(&"readme.txt".to_string()));

        let videos: Vec<String> = list_source_files(&dir, SourceKind::Video)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(videos, vec!["clip.mp4".to_string()]);
        fs::remove_dir_all(&dir).ok();
    }

    /// A file picked under `Score` becomes an `ImportInput::Score`, not a video.
    #[test]
    fn score_kind_builds_score_input() {
        let path = PathBuf::from("/tmp/scale.musicxml");
        assert!(matches!(
            SourceKind::Score.input_for(path.clone()),
            ImportInput::Score(p) if p == path
        ));
        assert!(matches!(
            SourceKind::Video.input_for(path.clone()),
            ImportInput::File(p) if p == path
        ));
    }

    #[test]
    fn score_picker_enter_selects_first() {
        let dir = temp_dir("spick_enter");
        touch(&dir, "alpha.musicxml");
        touch(&dir, "beta.abc");

        let mut picker = SourcePicker::new(dir.clone(), SourceKind::Score);
        assert_eq!(picker.kind(), SourceKind::Score);
        match picker.on_key(KeyCode::Enter).clone() {
            SourcePickerOutcome::Selected(p) => assert_eq!(
                p.file_name().unwrap().to_string_lossy().as_ref(),
                "alpha.musicxml"
            ),
            other => panic!("expected Selected, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    // ── Scan picker (M13-B) ───────────────────────────────────────────────────

    /// Scans and PDFs are offered by the same picker as score files: both go to
    /// the score sidecar, which decides for itself which one needs OMR.
    #[test]
    fn score_list_also_offers_scans_and_pdfs() {
        let dir = temp_dir("scan_ext");
        for name in [
            "page.pdf",
            "photo.JPG",
            "shot.jpeg",
            "sheet.png",
            "old.tif",
            "old2.tiff",
            "raw.bmp",
            "scale.musicxml",
        ] {
            touch(&dir, name);
        }
        touch(&dir, "clip.mp4");

        let names: Vec<String> = list_source_files(&dir, SourceKind::Score)
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        for expected in [
            "page.pdf",
            "photo.JPG",
            "shot.jpeg",
            "sheet.png",
            "old.tif",
            "old2.tiff",
            "raw.bmp",
            "scale.musicxml",
        ] {
            assert!(names.contains(&expected.to_string()), "missing {expected}");
        }
        assert!(
            !names.contains(&"clip.mp4".to_string()),
            "a video is not a scan"
        );
        fs::remove_dir_all(&dir).ok();
    }

    /// A scan picked under `Score` still becomes an `ImportInput::Score` — there
    /// is deliberately no third input kind, because the Rust side does not decide
    /// whether OMR runs.
    #[test]
    fn a_scan_uses_the_same_score_input() {
        let path = PathBuf::from("/tmp/page.pdf");
        assert!(matches!(
            SourceKind::Score.input_for(path.clone()),
            ImportInput::Score(p) if p == path
        ));
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

    /// The OMR confidence summary is promoted out of the log pane onto its own
    /// status line, while still remaining in the log with the engine's chatter.
    #[test]
    fn importing_screen_promotes_the_omr_summary_to_the_status_line() {
        let (tx, rx) = mpsc::channel();
        let mut screen = ImportingScreen::from_receiver(rx);
        assert_eq!(screen.summary(), None);

        for line in [
            "using OMR engine oemer: /usr/bin/oemer",
            "suspect measures (2): 12, 40",
            "omr: imported 412 notes, 37 flagged — review in the editor",
        ] {
            tx.send(WorkerEvent::Log(line.to_string())).unwrap();
        }
        screen.poll();

        assert_eq!(
            screen.summary(),
            Some("imported 412 notes, 37 flagged — review in the editor")
        );
        assert_eq!(screen.log.len(), 3, "the summary stays in the log too");
    }

    /// A score-file import reports no summary, so the extra status line never
    /// appears for a conversion that has nothing to doubt.
    #[test]
    fn importing_screen_has_no_summary_without_an_omr_line() {
        let (tx, rx) = mpsc::channel();
        let mut screen = ImportingScreen::from_receiver(rx);
        tx.send(WorkerEvent::Log(
            "dropped 1 grace note(s) (by design)".to_string(),
        ))
        .unwrap();
        screen.poll();
        assert_eq!(screen.summary(), None);
    }

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
        // With no fetch command available, a URL import must surface a clear
        // error — without ever spawning a fetch hook or touching the network.
        //
        // Isolation matters here: the pipeline resolves the fetch command from
        // `ROCKCRAFT_FETCH_CMD`, then falls back to `scripts/local/fetch.sh`
        // under the workspace root. On a developer machine that has set up
        // video import, that private (gitignored) script exists, so merely
        // clearing the env var is not enough — the probe would find the real
        // hook and the test would actually run yt-dlp against example.com.
        //
        // We therefore also point `ROCKCRAFT_WORKSPACE` at an empty temp dir so
        // the `scripts/local/fetch.sh` probe is guaranteed to find nothing,
        // exercising the genuine "no fetch command configured" path regardless
        // of ambient developer state.
        let empty_workspace = temp_dir("no_fetch_cmd_workspace");

        let prev_cmd = std::env::var("ROCKCRAFT_FETCH_CMD").ok();
        let prev_ws = std::env::var("ROCKCRAFT_WORKSPACE").ok();
        unsafe {
            std::env::remove_var("ROCKCRAFT_FETCH_CMD");
            std::env::set_var("ROCKCRAFT_WORKSPACE", &empty_workspace);
        }

        let mut screen =
            ImportingScreen::start(ImportInput::Url("https://example.com/video.mp4".into()));
        std::thread::sleep(std::time::Duration::from_millis(200));
        let outcome = screen.poll();
        let failed = matches!(outcome, Some(ImportOutcome::Failed(_)));

        // Restore env before asserting so a failure can't leak state.
        unsafe {
            match prev_cmd {
                Some(v) => std::env::set_var("ROCKCRAFT_FETCH_CMD", v),
                None => std::env::remove_var("ROCKCRAFT_FETCH_CMD"),
            }
            match prev_ws {
                Some(v) => std::env::set_var("ROCKCRAFT_WORKSPACE", v),
                None => std::env::remove_var("ROCKCRAFT_WORKSPACE"),
            }
        }
        fs::remove_dir_all(&empty_workspace).ok();

        assert!(failed, "expected Failed when no fetch command is available");
    }
}
