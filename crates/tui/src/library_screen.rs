//! The Library browser screen: lists saved/imported/recorded chart bundles and
//! opens the selected one in **Play** or **Edit**.
//!
//! The list is built by [`crate::library::list_library`]; each row shows the
//! name, note count, duration, origin and whether it has a backing track. The
//! screen is purely a selector — it returns a [`LibraryOutcome`] naming the
//! chosen bundle directory, and the shell reuses its existing bundle-load path
//! to enter Play or Edit.

use std::path::PathBuf;

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame,
};

use crate::library::{list_library, LibraryEntry};

/// What the user chose in the library browser.
#[derive(Debug, Clone, PartialEq)]
pub enum LibraryOutcome {
    /// Open the bundle at this directory in Play mode.
    OpenPlay(PathBuf),
    /// Open the bundle at this directory in Edit mode.
    OpenEdit(PathBuf),
    /// Esc — return to the menu, nothing opened.
    Cancelled,
    /// Still browsing; no decision yet.
    Pending,
}

/// The library browser overlay.
pub struct LibraryScreen {
    entries: Vec<LibraryEntry>,
    state: ListState,
    outcome: Option<LibraryOutcome>,
}

impl LibraryScreen {
    /// Build the browser by scanning `roots` for bundles.
    pub fn new(roots: &[PathBuf]) -> Self {
        let entries = list_library(roots);
        let mut state = ListState::default();
        if !entries.is_empty() {
            state.select(Some(0));
        }
        Self {
            entries,
            state,
            outcome: None,
        }
    }

    /// Number of bundles listed — for assertions in tests.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the library is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Feed a key press; returns the current [`LibraryOutcome`].
    ///
    /// `↑/↓/j/k` move; `Enter`/`p` open the selection in Play; `e` opens it in
    /// Edit; `Esc`/`q` cancel.
    pub fn on_key(&mut self, code: KeyCode) -> &LibraryOutcome {
        match code {
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Enter | KeyCode::Char('p') => {
                if let Some(dir) = self.selected_dir() {
                    self.outcome = Some(LibraryOutcome::OpenPlay(dir));
                }
            }
            KeyCode::Char('e') => {
                if let Some(dir) = self.selected_dir() {
                    self.outcome = Some(LibraryOutcome::OpenEdit(dir));
                }
            }
            KeyCode::Esc | KeyCode::Char('q') => {
                self.outcome = Some(LibraryOutcome::Cancelled);
            }
            _ => {}
        }
        self.outcome.as_ref().unwrap_or(&LibraryOutcome::Pending)
    }

    /// The directory of the currently highlighted bundle, if any.
    fn selected_dir(&self) -> Option<PathBuf> {
        let idx = self.state.selected()?;
        self.entries.get(idx).map(|e| e.dir.clone())
    }

    fn move_sel(&mut self, delta: isize) {
        if self.entries.is_empty() {
            return;
        }
        let n = self.entries.len() as isize;
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
            Paragraph::new(Line::from(
                "Library  — ↑/↓/j/k move · [Enter]/[p] Play · [e] Edit · [Esc] back",
            )),
            chunks[0],
        );

        let items: Vec<ListItem> = if self.entries.is_empty() {
            vec![ListItem::new(
                "(library empty — record, compose or import a track)",
            )]
        } else {
            self.entries
                .iter()
                .map(|e| ListItem::new(e.summary()))
                .collect()
        };

        let list = List::new(items)
            .block(Block::default().borders(Borders::ALL).title(" tracks "))
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
            Paragraph::new(Line::from(format!("{} track(s)", self.entries.len())))
                .style(Style::default().fg(Color::DarkGray)),
            chunks[2],
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{Grid, Timeline};

    /// Build a temp library root with one bundle saved via `EditScreen`.
    fn seed_one(name: &str) -> (PathBuf, PathBuf) {
        use crate::edit::EditScreen;
        use rockcraft_core::{MidiNote, Note, Velocity};

        let root = std::env::temp_dir().join(format!(
            "rockcraft_libscr_{}_{}",
            name,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let mut tl = Timeline::new();
        tl.insert(Note {
            pitch: MidiNote::new(60).unwrap(),
            start_us: 0,
            dur_us: 500_000,
            velocity: Velocity::new(80).unwrap(),
        });
        let edit = EditScreen::from_timeline(tl, Grid::default_120());
        let dir = edit.save_to_library(&root, name).expect("save");
        (root, dir)
    }

    #[test]
    fn lists_seeded_bundle_and_opens_play() {
        let (root, dir) = seed_one("mysong");
        let mut screen = LibraryScreen::new(std::slice::from_ref(&root));
        assert_eq!(screen.len(), 1, "the seeded bundle is listed");

        let outcome = screen.on_key(KeyCode::Enter).clone();
        assert_eq!(outcome, LibraryOutcome::OpenPlay(dir));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn e_opens_edit() {
        let (root, dir) = seed_one("song2");
        let mut screen = LibraryScreen::new(std::slice::from_ref(&root));
        let outcome = screen.on_key(KeyCode::Char('e')).clone();
        assert_eq!(outcome, LibraryOutcome::OpenEdit(dir));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn esc_cancels() {
        let mut screen = LibraryScreen::new(&[PathBuf::from("/no/such/library")]);
        assert!(screen.is_empty());
        let outcome = screen.on_key(KeyCode::Esc).clone();
        assert_eq!(outcome, LibraryOutcome::Cancelled);
    }
}
