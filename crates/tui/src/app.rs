//! The app shell: owns the live MIDI connection and switches between screens
//! (Menu / Record / Play). Each screen handles its own rendering; the shell
//! drains MIDI once per frame and routes events to the active screen.

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
    Frame, Terminal,
};
use rockcraft_audio::SynthHandle;
use rockcraft_midi::NoteSource;

use crate::play::PlayScreen;
use crate::record::RecordScreen;

/// Which screen is active.
enum Screen {
    Menu,
    Record(RecordScreen),
    Play(PlayScreen),
}

/// The menu entries, in order.
const MENU_ITEMS: &[&str] = &["Record", "Play last recording", "Quit"];

pub struct Shell {
    /// The swappable event source: real piano (`LiveInput`) or `MockKeyboard`.
    input: Box<dyn NoteSource>,
    /// Piano synth, if audio started. `None` runs silently.
    synth: Option<SynthHandle>,
    screen: Screen,
    menu_state: ListState,
    status: String,
    should_quit: bool,
}

impl Shell {
    pub fn new(input: Box<dyn NoteSource>, synth: Option<SynthHandle>) -> Self {
        let mut menu_state = ListState::default();
        menu_state.select(Some(0));
        Self {
            input,
            synth,
            screen: Screen::Menu,
            menu_state,
            status: String::new(),
            should_quit: false,
        }
    }

    fn menu_move(&mut self, delta: isize) {
        let n = MENU_ITEMS.len() as isize;
        let cur = self.menu_state.selected().unwrap_or(0) as isize;
        let next = (cur + delta).rem_euclid(n) as usize;
        self.menu_state.select(Some(next));
    }

    /// Act on the highlighted menu item.
    fn menu_activate(&mut self) {
        match self.menu_state.selected() {
            Some(0) => self.screen = Screen::Record(RecordScreen::new()),
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
            _ => self.should_quit = true,
        }
    }

    /// Handle a key press; returns to the menu on Tab from a screen.
    fn on_key(&mut self, code: KeyCode) {
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
        }
    }
}

/// Run the app shell until the user quits.
pub fn run(input: Box<dyn NoteSource>, synth: Option<SynthHandle>) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut shell = Shell::new(input, synth);
    let res = run_loop(&mut terminal, &mut shell);
    ratatui::restore();
    res
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    shell: &mut Shell,
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
                Screen::Menu => {}
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

        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    shell.on_key(key.code);
                }
            }
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

/// Find the most recent `take-*.mid` under `recordings/`, if any.
fn latest_recording() -> Option<std::path::PathBuf> {
    let dir = std::path::Path::new("recordings");
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "mid").unwrap_or(false))
        .collect();
    entries.sort();
    entries.pop()
}
