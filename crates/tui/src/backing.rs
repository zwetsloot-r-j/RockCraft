//! Backing-track file helper and in-app picker screen.
//!
//! `list_audio_files` is pure (no I/O beyond `std::fs`) and fully testable
//! in CI without an audio device.

use std::path::{Path, PathBuf};

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

// ---------------------------------------------------------------------------
// Pure helper
// ---------------------------------------------------------------------------

/// Audio extensions recognised by the picker (case-insensitive).
const AUDIO_EXTENSIONS: &[&str] = &["mp3", "wav", "ogg", "flac"];

/// Return audio files in `dir`, sorted by file name, skipping subdirectories
/// and non-audio files. Returns an empty `Vec` if `dir` is not readable.
pub fn list_audio_files(dir: &Path) -> Vec<PathBuf> {
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
                    .map(|e| AUDIO_EXTENSIONS.contains(&e.to_lowercase().as_str()))
                    .unwrap_or(false)
        })
        .collect();
    files.sort();
    files
}

// ---------------------------------------------------------------------------
// Picker screen
// ---------------------------------------------------------------------------

/// Result of a picker interaction.
#[derive(Debug, Clone, PartialEq)]
pub enum PickerOutcome {
    /// User pressed Enter on an entry.
    Selected(PathBuf),
    /// User pressed Esc — leave the prior selection unchanged.
    Cancelled,
    /// Still open; no decision yet.
    Pending,
}

/// An overlay that lets the user browse and select a backing audio file.
pub struct BackingPicker {
    dir: PathBuf,
    files: Vec<PathBuf>,
    state: ListState,
    outcome: Option<PickerOutcome>,
}

impl BackingPicker {
    /// Open the picker rooted at `dir`.  If a `backing/` subdirectory exists
    /// under `dir`, use that; otherwise use `dir` directly.
    pub fn new(dir: PathBuf) -> Self {
        let search_dir = if dir.join("backing").is_dir() {
            dir.join("backing")
        } else {
            dir.clone()
        };
        let files = list_audio_files(&search_dir);
        let mut state = ListState::default();
        if !files.is_empty() {
            state.select(Some(0));
        }
        Self {
            dir: search_dir,
            files,
            state,
            outcome: None,
        }
    }

    /// Feed a key press into the picker. Returns the current `PickerOutcome`.
    pub fn on_key(&mut self, code: KeyCode) -> &PickerOutcome {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Enter => {
                if let Some(idx) = self.state.selected() {
                    if let Some(path) = self.files.get(idx) {
                        self.outcome = Some(PickerOutcome::Selected(path.clone()));
                    }
                }
                // If list is empty, Enter cancels.
                if self.outcome.is_none() {
                    self.outcome = Some(PickerOutcome::Cancelled);
                }
            }
            KeyCode::Esc => {
                self.outcome = Some(PickerOutcome::Cancelled);
            }
            _ => {}
        }
        self.outcome.as_ref().unwrap_or(&PickerOutcome::Pending)
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

    /// Whether the picker has produced a final decision.
    pub fn is_done(&self) -> bool {
        self.outcome.is_some()
    }

    /// Consume the outcome (returns `Pending` if not yet decided).
    pub fn take_outcome(&mut self) -> PickerOutcome {
        self.outcome.take().unwrap_or(PickerOutcome::Pending)
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
                "Choose backing track  (dir: {})  — ↑/↓/j/k move · Enter select · Esc cancel",
                self.dir.display()
            ))),
            chunks[0],
        );

        let items: Vec<ListItem> = if self.files.is_empty() {
            vec![ListItem::new("(no audio files found)")]
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
                    .title(" audio files "),
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn make_temp_dir(suffix: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rockcraft_backing_{suffix}"));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn touch(dir: &Path, name: &str) {
        fs::write(dir.join(name), b"").unwrap();
    }

    // -----------------------------------------------------------------------
    // list_audio_files tests
    // -----------------------------------------------------------------------

    #[test]
    fn list_returns_only_audio_extensions() {
        let dir = make_temp_dir("audio_ext");
        touch(&dir, "track.mp3");
        touch(&dir, "song.WAV"); // uppercase extension
        touch(&dir, "readme.txt");
        touch(&dir, "image.png");
        fs::create_dir_all(dir.join("subdir")).unwrap();

        let files = list_audio_files(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert!(names.contains(&"track.mp3".to_string()));
        assert!(names.contains(&"song.WAV".to_string()));
        assert!(!names.contains(&"readme.txt".to_string()));
        assert!(!names.contains(&"image.png".to_string()));
        // subdir must not appear
        assert!(!names.iter().any(|n| n == "subdir"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_sorted_alphabetically() {
        let dir = make_temp_dir("sorted");
        touch(&dir, "zzz.mp3");
        touch(&dir, "aaa.wav");
        touch(&dir, "mmm.ogg");

        let files = list_audio_files(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["aaa.wav", "mmm.ogg", "zzz.mp3"]);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_empty_when_no_audio_files() {
        let dir = make_temp_dir("empty");
        touch(&dir, "readme.md");
        let files = list_audio_files(&dir);
        assert!(files.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn list_empty_on_nonexistent_dir() {
        let files = list_audio_files(Path::new("/nonexistent/path/that/does/not/exist"));
        assert!(files.is_empty());
    }

    #[test]
    fn list_skips_subdirectories() {
        let dir = make_temp_dir("subdirs");
        touch(&dir, "real.flac");
        // A directory named with an audio extension — must be skipped.
        fs::create_dir_all(dir.join("fake.mp3")).unwrap();

        let files = list_audio_files(&dir);
        let names: Vec<_> = files
            .iter()
            .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, vec!["real.flac"]);
        fs::remove_dir_all(&dir).ok();
    }

    // -----------------------------------------------------------------------
    // Picker navigation tests
    // -----------------------------------------------------------------------

    #[test]
    fn picker_enter_selects_first_item() {
        let dir = make_temp_dir("pick_enter");
        touch(&dir, "alpha.mp3");
        touch(&dir, "beta.wav");

        let mut picker = BackingPicker::new(dir.clone());
        let outcome = picker.on_key(KeyCode::Enter).clone();
        // First item (alpha.mp3 is sorted first) should be selected.
        match outcome {
            PickerOutcome::Selected(p) => {
                assert_eq!(
                    p.file_name().unwrap().to_string_lossy().as_ref(),
                    "alpha.mp3"
                );
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn picker_navigate_then_enter() {
        let dir = make_temp_dir("pick_nav");
        touch(&dir, "alpha.mp3");
        touch(&dir, "beta.wav");

        let mut picker = BackingPicker::new(dir.clone());
        picker.on_key(KeyCode::Down); // move to beta.wav
        let outcome = picker.on_key(KeyCode::Enter).clone();
        match outcome {
            PickerOutcome::Selected(p) => {
                assert_eq!(
                    p.file_name().unwrap().to_string_lossy().as_ref(),
                    "beta.wav"
                );
            }
            other => panic!("expected Selected, got {other:?}"),
        }
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn picker_esc_cancels() {
        let dir = make_temp_dir("pick_esc");
        touch(&dir, "track.mp3");

        let mut picker = BackingPicker::new(dir.clone());
        picker.on_key(KeyCode::Down); // move selection
        let outcome = picker.on_key(KeyCode::Esc).clone();
        assert_eq!(outcome, PickerOutcome::Cancelled);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn picker_esc_leaves_prior_selection_unchanged() {
        // After Esc the caller receives Cancelled and keeps whatever it had.
        let dir = make_temp_dir("pick_prior");
        touch(&dir, "track.mp3");

        let mut picker = BackingPicker::new(dir.clone());
        let outcome = picker.on_key(KeyCode::Esc).clone();
        assert_eq!(outcome, PickerOutcome::Cancelled);
        // The picker itself has no memory of external state; the caller is
        // responsible for leaving backing_path unchanged on Cancelled.
        fs::remove_dir_all(&dir).ok();
    }
}
