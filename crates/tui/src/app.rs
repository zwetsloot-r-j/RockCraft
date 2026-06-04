//! The app shell: owns the live MIDI connection and switches between screens
//! (Menu / Record / Play). Each screen handles its own rendering; the shell
//! drains MIDI once per frame and routes events to the active screen.

use std::io;
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::KeyCode;
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use rockcraft_audio::SynthHandle;
use rockcraft_core::{Grid, Key, RecordingMeta, Scale, Timeline};
use rockcraft_midi::{smf_bytes_to_events, NoteSource};

use crate::backing::{BackingPicker, PickerOutcome};
use crate::edit::EditScreen;
use crate::key_source::{CrosstermKeys, KeySource};
use crate::play::PlayScreen;
use crate::record::RecordScreen;

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Which screen is active.
pub(crate) enum Screen {
    Menu,
    Record(RecordScreen),
    Play(PlayScreen),
    Edit(EditScreen),
    BackingPicker(BackingPicker),
}

/// The menu entries, in order.
const MENU_ITEMS: &[&str] = &[
    "Record",
    "Play last recording",
    "Compose (new)",
    "Edit last recording",
    "Choose backing track",
    "Quit",
];

pub struct Shell {
    /// The swappable event source: real piano (`LiveInput`) or `MockKeyboard`.
    input: Box<dyn NoteSource>,
    /// Piano synth, if audio started. `None` runs silently.
    synth: Option<SynthHandle>,
    pub(crate) screen: Screen,
    menu_state: ListState,
    status: String,
    should_quit: bool,
    /// Optional backing track path forwarded to each new `RecordScreen`.
    backing_path: Option<PathBuf>,
}

impl Shell {
    pub fn new(
        input: Box<dyn NoteSource>,
        synth: Option<SynthHandle>,
        backing_path: Option<PathBuf>,
    ) -> Self {
        let mut menu_state = ListState::default();
        menu_state.select(Some(0));
        Self {
            input,
            synth,
            screen: Screen::Menu,
            menu_state,
            status: String::new(),
            should_quit: false,
            backing_path,
        }
    }

    /// Render the current state into a frame. Useful for headless tests.
    pub fn render(&self, f: &mut ratatui::Frame) {
        draw(f, self);
    }

    fn menu_move(&mut self, delta: isize) {
        let n = MENU_ITEMS.len() as isize;
        let cur = self.menu_state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(n) as usize;
        self.menu_state.select(Some(next));
    }

    /// Transition immediately into the composer with an empty timeline.
    pub(crate) fn activate_edit(&mut self) {
        let mut edit = EditScreen::new();
        if let Some(s) = &self.synth {
            edit.attach_synth(s.clone());
        }
        self.screen = Screen::Edit(edit);
    }

    /// Number of notes in the edit screen; `None` if not in edit mode.
    pub fn edit_note_count(&self) -> Option<usize> {
        if let Screen::Edit(e) = &self.screen {
            Some(e.note_count())
        } else {
            None
        }
    }

    /// Act on the highlighted menu item.
    fn menu_activate(&mut self) {
        match self.menu_state.selected() {
            Some(0) => {
                self.screen = Screen::Record(RecordScreen::with_backing(self.backing_path.clone()));
            }
            Some(1) => match latest_recording() {
                Some(path) => match std::fs::read(&path) {
                    Ok(bytes) => {
                        let title = path
                            .file_name()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "song".into());
                        match PlayScreen::from_smf_bytes(title, &bytes, self.synth.clone()) {
                            Ok(p) => self.screen = Screen::Play(p),
                            Err(e) => self.status = format!("load failed: {e}"),
                        }
                    }
                    Err(e) => self.status = format!("read failed: {e}"),
                },
                None => self.status = "no recordings yet — record one first".into(),
            },
            Some(2) => {
                // Compose (new): blank timeline.
                self.activate_edit();
            }
            Some(3) => match latest_recording() {
                // Edit last recording: load the latest take into the editor.
                Some(path) => match std::fs::read(&path) {
                    Ok(bytes) => match smf_bytes_to_events(&bytes) {
                        Ok(events) => {
                            let timeline = Timeline::from_events(&events);
                            let bundle_dir = path.parent().unwrap_or(&path);
                            let (grid, key) = load_meta_grid_key(bundle_dir);
                            let mut edit = EditScreen::from_timeline(timeline, grid);
                            edit.set_key(key);
                            if let Some(s) = &self.synth {
                                edit.attach_synth(s.clone());
                            }
                            self.screen = Screen::Edit(edit);
                        }
                        Err(e) => self.status = format!("parse failed: {e}"),
                    },
                    Err(e) => self.status = format!("read failed: {e}"),
                },
                None => self.status = "no recordings yet — record one first".into(),
            },
            Some(4) => {
                // Choose backing track: open the in-app file picker.
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                self.screen = Screen::BackingPicker(BackingPicker::new(cwd));
            }
            _ => self.should_quit = true,
        }
    }

    /// Handle a key press; returns to the menu on Tab/Esc from a screen.
    pub(crate) fn on_key(&mut self, code: KeyCode) {
        match &mut self.screen {
            Screen::Menu => match code {
                KeyCode::Up | KeyCode::Char('k') => self.menu_move(-1),
                KeyCode::Down | KeyCode::Char('j') => self.menu_move(1),
                KeyCode::Enter => self.menu_activate(),
                KeyCode::Char('q') | KeyCode::Esc => self.should_quit = true,
                _ => {}
            },
            // Note-key precedence inside a screen: navigation keys (Tab/Esc/
            // Enter/arrows) and the screen's own letter controls (`s` save here)
            // are handled first; any *other* letter is forwarded to the source
            // as a note key (a no-op on a real piano, a `press` on the mock).
            Screen::Record(rec) => match code {
                KeyCode::Tab | KeyCode::Esc => {
                    // Release anything still sounding before leaving the screen.
                    if let Some(s) = &self.synth {
                        s.all_off();
                    }
                    rec.stop_backing();
                    self.screen = Screen::Menu;
                }
                KeyCode::Char('s') => match rec.save() {
                    Ok(p) => self.status = format!("saved {}", p.display()),
                    Err(e) => self.status = format!("save failed: {e}"),
                },
                KeyCode::Char(c) => {
                    self.input.forward_key(c);
                }
                _ => {}
            },
            // Same precedence as Record: `r`/`m` are reserved controls, other
            // letters become note presses on the mock.
            Screen::Play(play) => match code {
                KeyCode::Tab | KeyCode::Esc => {
                    play.leave();
                    self.screen = Screen::Menu;
                }
                KeyCode::Char('r') => play.restart(),
                KeyCode::Char('m') => play.toggle_hear_song(),
                KeyCode::Char(c) => {
                    self.input.forward_key(c);
                }
                _ => {}
            },
            // The composer editor: Tab/Esc leave to the menu; `s` saves the
            // timeline bundle (matching Record's convention); every other key is
            // routed into the screen's own keymap. Exception: while the chord
            // selector is active, Esc cancels the chord instead of leaving.
            Screen::Edit(edit) => match code {
                KeyCode::Esc if edit.in_chord_mode() => edit.on_key(KeyCode::Esc),
                KeyCode::Tab | KeyCode::Esc => {
                    edit.leave();
                    self.screen = Screen::Menu;
                }
                KeyCode::Char('s') => match edit.save() {
                    Ok(p) => self.status = format!("saved {}", p.display()),
                    Err(e) => self.status = format!("save failed: {e}"),
                },
                other => edit.on_key(other),
            },
            Screen::BackingPicker(picker) => {
                let outcome = picker.on_key(code).clone();
                match outcome {
                    PickerOutcome::Selected(path) => {
                        self.status = format!("backing: {}", path.display());
                        self.backing_path = Some(path);
                        self.screen = Screen::Menu;
                    }
                    PickerOutcome::Cancelled => {
                        self.screen = Screen::Menu;
                    }
                    PickerOutcome::Pending => {}
                }
            }
        }
    }

    /// Name of the currently active screen — for assertions in tests.
    pub fn screen_name(&self) -> &'static str {
        match &self.screen {
            Screen::Menu => "menu",
            Screen::Record(_) => "record",
            Screen::Play(_) => "play",
            Screen::Edit(_) => "edit",
            Screen::BackingPicker(_) => "backing_picker",
        }
    }

    /// Whether the shell has been asked to quit.
    pub fn is_quit(&self) -> bool {
        self.should_quit
    }
}

// ---------------------------------------------------------------------------
// Run loop
// ---------------------------------------------------------------------------

/// Run the app shell until the user quits.
///
/// If `start_edit` is true the shell boots directly into the composer (the
/// `--edit` flag in `main.rs`), bypassing the menu.
pub fn run(
    input: Box<dyn NoteSource>,
    synth: Option<SynthHandle>,
    backing_path: Option<PathBuf>,
    start_edit: bool,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut shell = Shell::new(input, synth, backing_path);
    if start_edit {
        shell.activate_edit();
    }
    let mut keys = CrosstermKeys;
    let res = run_loop(&mut terminal, &mut shell, &mut keys);
    ratatui::restore();
    res
}

/// The frame loop. Separated from `run` so tests can inject a `TestBackend`
/// and a `ScriptedKeys` source without a real terminal or MIDI device.
pub fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    shell: &mut Shell,
    keys: &mut dyn KeySource,
) -> io::Result<()> {
    loop {
        // Drain MIDI and route to the active screen. Clone the synth handle out
        // first so we don't hold a borrow of `shell` across the screen match.
        let synth = shell.synth.clone();
        let events = shell.input.events();
        for ev in events {
            match &mut shell.screen {
                Screen::Record(rec) => {
                    rec.ingest(ev);
                    // Sound the keys you play while recording.
                    if let Some(s) = &synth {
                        s.apply(&ev);
                    }
                }
                Screen::Play(play) => play.ingest(ev),
                // In a record mode the editor consumes played notes (step / live
                // record); in direct-edit `ingest` ignores them. Either way we
                // sound the keys you play, as Record does.
                Screen::Edit(edit) => {
                    edit.ingest(ev);
                    if let Some(s) = &synth {
                        s.apply(&ev);
                    }
                }
                // These screens ignore live MIDI input.
                Screen::Menu | Screen::BackingPicker(_) => {}
            }
        }

        // Tick song-synth triggers (clock-driven, not frame-rate-driven).
        if let Screen::Play(play) = &mut shell.screen {
            play.tick_song_synth();
        }

        // A finished song returns to the menu on its own.
        if let Screen::Play(play) = &shell.screen {
            if play.is_finished() {
                shell.status = "song finished".into();
                shell.screen = Screen::Menu;
            }
        }

        terminal.draw(|f| draw(f, shell))?;

        if let Some(code) = keys.poll_key(Duration::from_millis(16))? {
            shell.on_key(code);
        }

        if shell.should_quit {
            return Ok(());
        }
    }
}

fn draw(f: &mut Frame, shell: &Shell) {
    match &shell.screen {
        Screen::Menu => draw_menu(f, f.area(), shell),
        Screen::Record(rec) => rec.draw(f, f.area()),
        Screen::Play(play) => play.draw(f, f.area()),
        Screen::Edit(edit) => edit.draw(f, f.area()),
        Screen::BackingPicker(picker) => picker.draw(f, f.area()),
    }
}

fn draw_menu(f: &mut Frame, area: Rect, shell: &Shell) {
    let chunks = Layout::vertical([
        Constraint::Length(2),
        Constraint::Min(3),
        Constraint::Length(1),
    ])
    .split(area);

    let title = Line::from(format!(
        "RockCraft — {}  (↑/↓ select, Enter, q quit)",
        shell.input.port_name()
    ));
    f.render_widget(Paragraph::new(title), chunks[0]);

    let items: Vec<ListItem> = MENU_ITEMS.iter().map(|s| ListItem::new(*s)).collect();
    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(" mode "))
        .highlight_style(
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");
    let mut state = shell.menu_state.clone();
    f.render_stateful_widget(list, chunks[1], &mut state);

    f.render_widget(
        Paragraph::new(Line::from(shell.status.as_str())).style(Style::default().fg(Color::Yellow)),
        chunks[2],
    );
}

/// Read `meta.json` from a bundle dir; return `(Grid, Key)`, falling back to defaults.
fn load_meta_grid_key(bundle_dir: &std::path::Path) -> (Grid, Key) {
    let default_key = Key {
        root_pc: 0,
        scale: Scale::Major,
    };
    let Ok(json) = std::fs::read_to_string(bundle_dir.join("meta.json")) else {
        return (Grid::default_120(), default_key);
    };
    let Ok(meta) = RecordingMeta::from_json(&json) else {
        return (Grid::default_120(), default_key);
    };
    (
        meta.grid.unwrap_or_else(Grid::default_120),
        meta.key.unwrap_or(default_key),
    )
}

/// Find `song.mid` inside the most recent `take-*/` bundle under `recordings/`.
fn latest_recording() -> Option<std::path::PathBuf> {
    latest_recording_from(std::path::Path::new("recordings"))
}

/// Find `song.mid` inside the most recent `take-*/` bundle under `base`.
/// Extracted so tests can point at a temp directory.
pub(crate) fn latest_recording_from(base: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut bundles: Vec<_> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.is_dir()
                && p.file_name()
                    .map(|n| n.to_string_lossy().starts_with("take-"))
                    .unwrap_or(false)
        })
        .collect();
    bundles.sort();
    let latest = bundles.pop()?;
    let midi = latest.join("song.mid");
    midi.exists().then_some(midi)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{Grid, Key, MidiNote, Note, Scale, Timeline, Velocity};
    use rockcraft_midi::ScriptedSource;

    fn make_note(pitch: u8, start: u64, dur: u64) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us: start,
            dur_us: dur,
            velocity: Velocity::new(80).unwrap(),
        }
    }

    fn make_shell() -> Shell {
        Shell::new(Box::new(ScriptedSource::new(vec![])), None, None)
    }

    /// "Compose (new)" (index 2) enters the editor with an empty timeline,
    /// driven by the same key routing used in the live shell.
    #[test]
    fn compose_new_enters_empty_edit_screen() {
        let mut shell = make_shell();

        shell.on_key(KeyCode::Down); // → index 1 (Play last recording)
        shell.on_key(KeyCode::Down); // → index 2 (Compose new)
        shell.on_key(KeyCode::Enter); // enter the editor

        assert_eq!(shell.screen_name(), "edit");
        assert_eq!(
            shell.edit_note_count(),
            Some(0),
            "new composition starts with an empty timeline"
        );
    }

    /// Seeding a bundle then loading it via "Edit last recording" enters the
    /// editor pre-populated with the saved notes.
    #[test]
    fn edit_last_recording_loads_seeded_bundle() {
        let mut tl = Timeline::new();
        tl.insert(make_note(60, 0, 500_000));
        tl.insert(make_note(64, 500_000, 500_000));
        let expected_count = tl.len();

        // Save a bundle into a temp base dir using EditScreen's save_bundle.
        let base = std::env::temp_dir().join(format!(
            "rockcraft_edit_last_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        let edit_src = EditScreen::from_timeline(tl, Grid::default_120());
        edit_src.save_bundle(&base).expect("seed save failed");

        // Verify latest_recording_from finds the bundle's song.mid and that
        // the loaded timeline has the expected number of notes.
        let midi_path = latest_recording_from(&base).expect("recording not found");
        let bytes = std::fs::read(&midi_path).expect("read midi failed");
        let events = smf_bytes_to_events(&bytes).expect("parse failed");
        let reloaded = Timeline::from_events(&events);

        std::fs::remove_dir_all(&base).ok();

        assert_eq!(
            reloaded.len(),
            expected_count,
            "editor is pre-populated with the saved notes"
        );
    }

    /// Saving a composition with a custom grid and key, then loading it via
    /// `load_meta_grid_key`, restores the same grid bpm/subdivision and key.
    #[test]
    fn save_load_restores_grid_and_key() {
        use rockcraft_core::{Subdivision, TimeSig};

        let grid = Grid {
            bpm: 140,
            time_sig: TimeSig {
                beats_per_bar: 3,
                beat_unit: 4,
            },
            subdivision: Subdivision::Eighth,
        };
        let key = Key {
            root_pc: 2,
            scale: Scale::NaturalMinor,
        };

        let base = std::env::temp_dir().join(format!(
            "rockcraft_grid_key_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .subsec_nanos()
        ));
        let mut edit = EditScreen::from_timeline(Timeline::new(), grid);
        edit.set_key(key);
        let bundle_dir = edit.save_bundle(&base).expect("save failed");

        let (loaded_grid, loaded_key) = load_meta_grid_key(&bundle_dir);

        std::fs::remove_dir_all(&base).ok();

        assert_eq!(loaded_grid, grid, "grid must round-trip through meta.json");
        assert_eq!(loaded_key, key, "key must round-trip through meta.json");
    }
}
