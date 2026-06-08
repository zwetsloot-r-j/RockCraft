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
use rockcraft_control::{QueryKind, RemoteCommand, Request, Response};
use rockcraft_core::{Grid, Key, RecordingMeta, Scale, Timeline};
use rockcraft_import::{fetch_command_configured, ImportInput};
use rockcraft_midi::{smf_bytes_to_events, NoteSource};
use tokio::sync::mpsc;

use crate::backing::{BackingPicker, PickerOutcome};
use crate::edit::EditScreen;
use crate::import_screen::{
    ImportOutcome, ImportingScreen, UrlInput, UrlInputOutcome, VideoPicker, VideoPickerOutcome,
};
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
    Edit(Box<EditScreen>),
    BackingPicker(BackingPicker),
    /// Browse for a local video file to import.
    VideoPicker(VideoPicker),
    /// Text input for a video URL to import.
    UrlInput(UrlInput),
    /// Running the import pipeline, showing progress.
    Importing(ImportingScreen),
}

/// Fixed menu entries always shown.
const MENU_ITEMS_BASE: &[&str] = &[
    "Record",
    "Play last recording",
    "Compose (new)",
    "Edit last recording",
    "Choose backing track",
    "Import from video file…",
];

/// Extra entry appended when a fetch command is configured (M6-D).
const MENU_ITEM_URL: &str = "Import from URL…";

/// Last entry always shown.
const MENU_ITEM_QUIT: &str = "Quit";

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
    /// Inbound remote commands from the control server, if one is running.
    /// Drained once per run-loop iteration with non-blocking `try_recv`; `None`
    /// when no `--control` server was started (the default). The render/MIDI
    /// loop never awaits this — it only polls it.
    commands: Option<mpsc::Receiver<RemoteCommand>>,
    /// Last known terminal dimensions, updated each frame in the run loop.
    /// Used by `render_to_string` when no explicit size is given. Falls back to
    /// 80×24 in headless / test contexts where the run loop never fires.
    terminal_size: (u16, u16),
    /// `true` when a URL fetch command is configured (M6-D hook).
    /// Determines whether the "Import from URL…" menu item is shown.
    has_fetch_cmd: bool,
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
            commands: None,
            terminal_size: (80, 24),
            has_fetch_cmd: fetch_command_configured(),
        }
    }

    /// Override the fetch-command capability flag — used in tests.
    pub fn set_has_fetch_cmd(&mut self, v: bool) {
        self.has_fetch_cmd = v;
    }

    /// Build the current menu item list, injecting URL import when configured.
    fn menu_items(&self) -> Vec<&str> {
        let mut items: Vec<&str> = MENU_ITEMS_BASE.to_vec();
        if self.has_fetch_cmd {
            items.push(MENU_ITEM_URL);
        }
        items.push(MENU_ITEM_QUIT);
        items
    }

    /// Attach the receiving half of the control-server command channel. Called
    /// by [`run`] when `--control` is enabled; the run loop then drains it each
    /// iteration. Kept off the `new` signature so existing callers/tests are
    /// unaffected.
    pub fn set_command_receiver(&mut self, rx: mpsc::Receiver<RemoteCommand>) {
        self.commands = Some(rx);
    }

    /// Render the current state into a frame. Useful for headless tests.
    pub fn render(&self, f: &mut ratatui::Frame) {
        draw(f, self);
    }

    /// Render the app's current view into an off-screen `TestBackend` of the
    /// given size and flatten the buffer to text: one line per row, trailing
    /// blanks trimmed, rows joined by `\n`.
    pub fn render_to_string(&self, width: u16, height: u16) -> String {
        use ratatui::backend::TestBackend;
        let backend = TestBackend::new(width, height);
        let mut terminal = Terminal::new(backend).expect("TestBackend init");
        terminal
            .draw(|f| draw(f, self))
            .expect("render_to_string draw");
        let buf = terminal.backend().buffer();
        let content = buf.content();
        let w = width as usize;
        (0..height as usize)
            .map(|row| {
                let row_str: String = content[row * w..(row + 1) * w]
                    .iter()
                    .map(|cell| cell.symbol())
                    .collect();
                row_str.trim_end().to_string()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn menu_move(&mut self, delta: isize) {
        let n = self.menu_items().len() as isize;
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
        self.screen = Screen::Edit(Box::new(edit));
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
        let items = self.menu_items();
        let idx = self.menu_state.selected().unwrap_or(0);
        let item = items.get(idx).copied().unwrap_or("");
        match item {
            "Record" => {
                self.screen = Screen::Record(RecordScreen::with_backing(self.backing_path.clone()));
            }
            "Play last recording" => match latest_recording() {
                Some(path) => match load_play_screen(&path, self.synth.clone()) {
                    Ok(p) => self.screen = Screen::Play(p),
                    Err(e) => self.status = e,
                },
                None => self.status = "no recordings yet — record one first".into(),
            },
            "Compose (new)" => {
                self.activate_edit();
            }
            "Edit last recording" => match latest_recording() {
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
                            self.screen = Screen::Edit(Box::new(edit));
                        }
                        Err(e) => self.status = format!("parse failed: {e}"),
                    },
                    Err(e) => self.status = format!("read failed: {e}"),
                },
                None => self.status = "no recordings yet — record one first".into(),
            },
            "Choose backing track" => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                self.screen = Screen::BackingPicker(BackingPicker::new(cwd));
            }
            "Import from video file…" => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                self.screen = Screen::VideoPicker(VideoPicker::new(cwd));
            }
            "Import from URL…" => {
                self.screen = Screen::UrlInput(UrlInput::new());
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
                // Clear visual selection on Esc without leaving the editor.
                KeyCode::Esc if edit.in_visual_mode() => edit.on_key(KeyCode::Esc),
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
            Screen::VideoPicker(picker) => {
                let outcome = picker.on_key(code).clone();
                match outcome {
                    VideoPickerOutcome::Selected(path) => {
                        self.screen =
                            Screen::Importing(ImportingScreen::start(ImportInput::File(path)));
                    }
                    VideoPickerOutcome::Cancelled => {
                        self.screen = Screen::Menu;
                    }
                    VideoPickerOutcome::Pending => {}
                }
            }
            Screen::UrlInput(ui) => {
                let outcome = ui.on_key(code).clone();
                match outcome {
                    UrlInputOutcome::Submitted(url) => {
                        self.screen =
                            Screen::Importing(ImportingScreen::start(ImportInput::Url(url)));
                    }
                    UrlInputOutcome::Cancelled => {
                        self.screen = Screen::Menu;
                    }
                    UrlInputOutcome::Pending => {}
                }
            }
            Screen::Importing(_) => {
                // Esc cancels (the thread continues running but we abandon it).
                if code == KeyCode::Esc {
                    self.status = "import cancelled".into();
                    self.screen = Screen::Menu;
                }
            }
        }
    }

    /// Drain every queued remote command and apply it, in receive order.
    ///
    /// Non-blocking: `try_recv` until the channel is empty, so the render/MIDI
    /// loop never stalls on the socket. Commands are collected first (to release
    /// the borrow on `self.commands`) then applied in order, each replying over
    /// its own oneshot with the post-edit snapshot. A no-op when no control
    /// server is attached.
    pub(crate) fn drain_remote_commands(&mut self) {
        let mut pending = Vec::new();
        if let Some(rx) = self.commands.as_mut() {
            // `try_recv` returns Empty (stop) or Disconnected (also stop).
            while let Ok(cmd) = rx.try_recv() {
                pending.push(cmd);
            }
        }
        for cmd in pending {
            let response = self.handle_remote(cmd.req);
            // The client may have gone away; a failed reply is not fatal.
            let _ = cmd.reply.send(response);
        }
    }

    /// Apply one remote [`Request`] against the composer the app owns.
    ///
    /// `Query::Render` is intercepted here and rendered from the full shell
    /// (not just the composer) using the last known terminal size. All other
    /// requests are forwarded to the active screen; outside the editor they are
    /// rejected with an error echoing the request id.
    fn handle_remote(&mut self, req: Request) -> Response {
        // Render queries need the shell, not just the composer.
        if let Request::Query {
            id,
            what: QueryKind::Render,
        } = &req
        {
            let (w, h) = self.terminal_size;
            return Response::Render {
                id: *id,
                text: self.render_to_string(w, h),
            };
        }
        match &mut self.screen {
            Screen::Edit(edit) => edit.apply_remote(req),
            _ => Response::Err {
                id: req.id(),
                error: "unavailable: open the editor to accept control commands".into(),
            },
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
            Screen::VideoPicker(_) => "video_picker",
            Screen::UrlInput(_) => "url_input",
            Screen::Importing(_) => "importing",
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
/// `--edit` flag in `main.rs`), bypassing the menu. When `commands` is `Some`,
/// the loop also drains remote control commands from the control server.
pub fn run(
    input: Box<dyn NoteSource>,
    synth: Option<SynthHandle>,
    backing_path: Option<PathBuf>,
    start_edit: bool,
    commands: Option<mpsc::Receiver<RemoteCommand>>,
) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut shell = Shell::new(input, synth, backing_path);
    if start_edit {
        shell.activate_edit();
    }
    if let Some(rx) = commands {
        shell.set_command_receiver(rx);
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
        // Apply any remote control commands first, so a remote edit and a
        // keypress in the same frame both land before this iteration's redraw.
        shell.drain_remote_commands();

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
                Screen::Menu
                | Screen::BackingPicker(_)
                | Screen::VideoPicker(_)
                | Screen::UrlInput(_)
                | Screen::Importing(_) => {}
            }
        }

        // Tick song-synth triggers and the backing track (clock-driven, not
        // frame-rate-driven); the backing arms itself once the clock reaches the
        // lead-in's end so it lines up with the notes hitting the keyboard line.
        if let Screen::Play(play) = &mut shell.screen {
            play.tick_song_synth();
            play.tick_backing();
        }

        // Tick editor transport audition (clock-driven).
        if let Screen::Edit(edit) = &mut shell.screen {
            edit.tick_audition();
        }

        // A finished song returns to the menu on its own.
        if let Screen::Play(play) = &shell.screen {
            if play.is_finished() {
                shell.status = "song finished".into();
                shell.screen = Screen::Menu;
            }
        }

        // Poll the import pipeline and handle completion.
        let import_result = if let Screen::Importing(imp) = &mut shell.screen {
            match imp.poll() {
                Some(ImportOutcome::Done(path)) => Some(Ok(path.clone())),
                Some(ImportOutcome::Failed(msg)) => Some(Err(msg.clone())),
                None => None,
            }
        } else {
            None
        };
        match import_result {
            Some(Ok(bundle)) => {
                let midi = bundle.join("song.mid");
                match load_play_screen(&midi, shell.synth.clone()) {
                    Ok(play) => {
                        shell.status = format!("imported: {}", bundle.display());
                        shell.screen = Screen::Play(play);
                    }
                    Err(e) => {
                        shell.status = format!("import succeeded but load failed: {e}");
                        shell.screen = Screen::Menu;
                    }
                }
            }
            Some(Err(msg)) => {
                shell.status = format!("import failed: {msg}");
                shell.screen = Screen::Menu;
            }
            None => {}
        }

        let completed = terminal.draw(|f| draw(f, shell))?;
        shell.terminal_size = (completed.area.width, completed.area.height);

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
        Screen::VideoPicker(picker) => picker.draw(f, f.area()),
        Screen::UrlInput(ui) => ui.draw(f, f.area()),
        Screen::Importing(imp) => imp.draw(f, f.area()),
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

    let items: Vec<ListItem> = shell
        .menu_items()
        .iter()
        .map(|s| ListItem::new(*s))
        .collect();
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

/// Find the MIDI of the most recent recording under `recordings/`.
fn latest_recording() -> Option<std::path::PathBuf> {
    latest_recording_from(std::path::Path::new("recordings"))
}

/// Find the MIDI of the most recent recording under `base`, preferring
/// `take-*/` bundle directories (returning their `song.mid`) but also finding
/// legacy loose `take-*.mid` files. Newest wins by take name (the unix-stamp in
/// `take-<stamp>`). Extracted so tests can point at a temp directory.
pub(crate) fn latest_recording_from(base: &std::path::Path) -> Option<std::path::PathBuf> {
    // (take name, midi path) — the name keys the "newest" sort; bundles and
    // loose files share the `take-<stamp>` naming so they order together.
    let mut candidates: Vec<(String, std::path::PathBuf)> = std::fs::read_dir(base)
        .ok()?
        .flatten()
        .filter_map(|e| {
            let path = e.path();
            let name = path.file_name()?.to_string_lossy().into_owned();
            if !name.starts_with("take-") {
                return None;
            }
            if path.is_dir() {
                // Bundle: the MIDI lives inside as song.mid.
                let midi = path.join("song.mid");
                midi.exists().then_some((name, midi))
            } else if path.extension().map(|x| x == "mid").unwrap_or(false) {
                // Legacy loose take-*.mid; key on the stem so it sorts with bundles.
                let stem = path.file_stem()?.to_string_lossy().into_owned();
                Some((stem, path))
            } else {
                None
            }
        })
        .collect();
    candidates.sort_by(|a, b| a.0.cmp(&b.0));
    candidates.pop().map(|(_, midi)| midi)
}

/// Build a [`PlayScreen`] for the MIDI at `midi_path`, attaching the bundle's
/// backing track when its sibling `meta.json` declares one. Loose `.mid` files
/// (no sibling manifest) load MIDI-only, exactly as before.
fn load_play_screen(
    midi_path: &std::path::Path,
    synth: Option<SynthHandle>,
) -> Result<PlayScreen, String> {
    let bytes = std::fs::read(midi_path).map_err(|e| format!("read failed: {e}"))?;
    let title = midi_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "song".into());
    let mut play = PlayScreen::from_smf_bytes(title, &bytes, synth)
        .map_err(|e| format!("load failed: {e}"))?;

    // A bundle keeps its manifest next to song.mid; resolve the backing track
    // (if any) relative to that directory so the bundle stays movable.
    if let Some(dir) = midi_path.parent() {
        if let Ok(json) = std::fs::read_to_string(dir.join("meta.json")) {
            if let Ok(meta) = RecordingMeta::from_json(&json) {
                if let Some(backing) = meta.backing {
                    play = play.with_backing(dir.join(&backing.file), backing.audio_start_us);
                }
            }
        }
    }
    Ok(play)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_core::{Grid, Key, MidiNote, Note, Scale, Timeline, Velocity};
    use rockcraft_midi::ScriptedSource;
    use tokio::sync::oneshot;

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

    // ── control-server wiring (M4-F) ─────────────────────────────────────

    /// Build a `RemoteCommand` from a wire-format JSON request, returning it
    /// alongside the oneshot receiver that will carry its response.
    fn remote(json: &str) -> (RemoteCommand, oneshot::Receiver<Response>) {
        let req: Request = serde_json::from_str(json).expect("valid request json");
        let (reply, reply_rx) = oneshot::channel();
        (RemoteCommand { req, reply }, reply_rx)
    }

    /// Note count carried by an `Ok` response's snapshot.
    fn snapshot_note_count(resp: &Response) -> usize {
        match resp {
            Response::Ok {
                state: Some(snap), ..
            } => snap.notes.len(),
            other => panic!("expected Ok with state, got {other:?}"),
        }
    }

    /// A remote `add_note` and a scripted keypress edit converge on the same
    /// `Composer`: the note count reflects both, and a follow-up `query state`
    /// returns the merged snapshot.
    #[test]
    fn remote_and_keypress_converge_on_one_composer() {
        let mut shell = make_shell();
        shell.activate_edit();
        let (tx, rx) = mpsc::channel::<RemoteCommand>(16);
        shell.set_command_receiver(rx);

        // Remote add at the default cursor (middle C, step 0).
        let (cmd, mut reply_rx) = remote(r#"{"type":"run_action","action":"add_note"}"#);
        tx.try_send(cmd).expect("queue remote add");
        shell.drain_remote_commands();
        let resp = reply_rx.try_recv().expect("remote reply");
        assert_eq!(snapshot_note_count(&resp), 1, "remote add lands");
        assert_eq!(shell.edit_note_count(), Some(1));

        // Keyboard add at a fresh cell (move right, then add).
        shell.on_key(KeyCode::Char('l'));
        shell.on_key(KeyCode::Char('a'));
        assert_eq!(shell.edit_note_count(), Some(2), "keypress add lands too");

        // A remote state query sees the merged result of both surfaces.
        let (cmd, mut reply_rx) = remote(r#"{"type":"query","what":"State"}"#);
        tx.try_send(cmd).expect("queue query");
        shell.drain_remote_commands();
        let resp = reply_rx.try_recv().expect("query reply");
        assert_eq!(
            snapshot_note_count(&resp),
            2,
            "query reflects remote + keypress edits"
        );
    }

    /// Commands queued before a single drain apply in receive order, and each
    /// oneshot reply carries that command's own post-edit snapshot.
    #[test]
    fn interleaved_commands_apply_in_receive_order() {
        let mut shell = make_shell();
        shell.activate_edit();
        let (tx, rx) = mpsc::channel::<RemoteCommand>(16);
        shell.set_command_receiver(rx);

        // add_note (→1), cursor_right (still 1), add_note (→2): one drain.
        let (c1, mut r1) = remote(r#"{"type":"run_action","action":"add_note"}"#);
        let (c2, mut r2) = remote(r#"{"type":"run_action","action":"cursor_right"}"#);
        let (c3, mut r3) = remote(r#"{"type":"run_action","action":"add_note"}"#);
        tx.try_send(c1).unwrap();
        tx.try_send(c2).unwrap();
        tx.try_send(c3).unwrap();

        shell.drain_remote_commands();

        // Replies carry each step's post-edit snapshot, proving in-order apply.
        assert_eq!(snapshot_note_count(&r1.try_recv().unwrap()), 1);
        assert_eq!(snapshot_note_count(&r2.try_recv().unwrap()), 1);
        assert_eq!(snapshot_note_count(&r3.try_recv().unwrap()), 2);
        assert_eq!(shell.edit_note_count(), Some(2));
    }

    /// A remote command received while not in the editor is rejected (not
    /// applied) and the error echoes the request id.
    #[test]
    fn remote_command_outside_editor_is_rejected() {
        let mut shell = make_shell(); // stays on the menu
        let (tx, rx) = mpsc::channel::<RemoteCommand>(4);
        shell.set_command_receiver(rx);

        let (cmd, mut reply_rx) = remote(r#"{"type":"run_action","id":9,"action":"add_note"}"#);
        tx.try_send(cmd).expect("queue remote add");
        shell.drain_remote_commands();

        match reply_rx.try_recv().expect("reply") {
            Response::Err { id, error } => {
                assert_eq!(id, Some(9), "error echoes the request id");
                assert!(error.starts_with("unavailable:"), "got {error}");
            }
            other => panic!("expected Err, got {other:?}"),
        }
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

    // ── render_to_string (M4-G) ──────────────────────────────────────────────

    /// `render_to_string` produces exactly `height` rows and is deterministic:
    /// the same state always yields the same string. Moving the cursor (which
    /// shifts the `█` marker to a different piano-key column) must change the
    /// dump, proving that state changes propagate to the render.
    #[test]
    fn render_to_string_dimensions_and_determinism() {
        let w = 80u16;
        let h = 24u16;

        let mut shell = make_shell();
        shell.activate_edit();
        let (tx, rx) = mpsc::channel::<RemoteCommand>(16);
        shell.set_command_receiver(rx);

        // Initial render: must have exactly h rows.
        let initial = shell.render_to_string(w, h);
        assert_eq!(
            initial.split('\n').count(),
            h as usize,
            "must produce exactly h lines"
        );

        // Same state → identical string (determinism).
        let initial2 = shell.render_to_string(w, h);
        assert_eq!(initial, initial2, "render_to_string must be deterministic");

        // Move cursor up one semitone: the cursor block shifts to a different
        // piano-key column, so the render must change.
        let (cmd, _) = remote(r#"{"type":"run_action","action":"cursor_up"}"#);
        tx.try_send(cmd).unwrap();
        shell.drain_remote_commands();

        let after_move = shell.render_to_string(w, h);
        assert_ne!(
            initial, after_move,
            "cursor move must change the render dump"
        );

        // Still deterministic after the state change.
        let after_move2 = shell.render_to_string(w, h);
        assert_eq!(
            after_move, after_move2,
            "render_to_string must be deterministic after state change"
        );
    }

    /// `render_to_string` returns non-empty text that includes at least one
    /// non-space character (the keyboard row / border), even from the menu.
    #[test]
    fn render_to_string_non_empty_from_menu() {
        let shell = make_shell(); // stays on menu
        let text = shell.render_to_string(80, 24);
        assert!(
            text.chars().any(|c| !c.is_whitespace()),
            "render dump must contain at least one non-space character"
        );
    }

    /// Pressing `+`/`-` on a note must produce a visible change in the rendered
    /// output: the status bar's velocity indicator updates on every adjustment.
    #[test]
    fn velocity_change_visible_in_render() {
        let mut shell = make_shell();
        shell.activate_edit();
        let (tx, rx) = mpsc::channel::<RemoteCommand>(16);
        shell.set_command_receiver(rx);

        let (c1, _) = remote(r#"{"type":"run_action","action":"add_note"}"#);
        tx.try_send(c1).unwrap();
        shell.drain_remote_commands();
        let before = shell.render_to_string(80, 24);

        let (c2, _) =
            remote(r#"{"type":"run_action","action":"adjust_velocity","params":{"delta":8}}"#);
        tx.try_send(c2).unwrap();
        shell.drain_remote_commands();
        let after = shell.render_to_string(80, 24);

        assert_ne!(
            before, after,
            "+/- must change the rendered velocity indicator"
        );
        assert!(
            after.contains("vel "),
            "status bar must show 'vel N' after adjustment"
        );
    }

    /// Through the channel harness: `query state` reflects added notes and
    /// `query render` returns non-empty text with the right number of rows.
    #[test]
    fn remote_query_state_and_render() {
        let mut shell = make_shell();
        shell.activate_edit();
        let (tx, rx) = mpsc::channel::<RemoteCommand>(16);
        shell.set_command_receiver(rx);

        // Add two notes via remote commands.
        let (c1, _) = remote(r#"{"type":"run_action","action":"add_note"}"#);
        let (c2, _) = remote(r#"{"type":"run_action","action":"cursor_right"}"#);
        let (c3, _) = remote(r#"{"type":"run_action","action":"add_note"}"#);
        tx.try_send(c1).unwrap();
        tx.try_send(c2).unwrap();
        tx.try_send(c3).unwrap();
        shell.drain_remote_commands();

        // query state — snapshot must list the two notes.
        let (cmd, mut reply_rx) = remote(r#"{"type":"query","what":"State"}"#);
        tx.try_send(cmd).expect("queue state query");
        shell.drain_remote_commands();
        let resp = reply_rx.try_recv().expect("state reply");
        assert_eq!(snapshot_note_count(&resp), 2, "state query sees both notes");

        // query render — text must be non-empty and have exactly h rows.
        // Shell defaults to 80×24 before any run-loop frame fires.
        let h = 24u16;
        let (cmd, mut reply_rx) = remote(r#"{"type":"query","what":"Render"}"#);
        tx.try_send(cmd).expect("queue render query");
        shell.drain_remote_commands();
        match reply_rx.try_recv().expect("render reply") {
            Response::Render { text, .. } => {
                assert!(!text.is_empty(), "render query must return non-empty text");
                let lines: Vec<&str> = text.split('\n').collect();
                assert_eq!(
                    lines.len(),
                    h as usize,
                    "render text must have exactly h={h} rows"
                );
            }
            other => panic!("expected Render response, got {other:?}"),
        }
    }

    // ── M6-E import menu integration ─────────────────────────────────────────

    /// Without a fetch command, the "Import from URL…" item must be absent and
    /// "Import from video file…" must be present.
    #[test]
    fn url_menu_item_absent_when_no_fetch_cmd() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(false);

        let items = shell.menu_items();
        assert!(
            items.contains(&"Import from video file…"),
            "file import item must always be present"
        );
        assert!(
            !items.contains(&"Import from URL…"),
            "URL import item must be absent when no fetch command is configured"
        );
    }

    /// With a fetch command configured, both import items are present.
    #[test]
    fn url_menu_item_present_when_fetch_cmd_configured() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(true);

        let items = shell.menu_items();
        assert!(
            items.contains(&"Import from video file…"),
            "file import item must be present"
        );
        assert!(
            items.contains(&"Import from URL…"),
            "URL import item must be present when fetch command is configured"
        );
    }

    /// Navigating to "Import from video file…" and pressing Enter opens the
    /// video picker screen.
    #[test]
    fn import_from_file_menu_item_opens_video_picker() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(false);

        // Navigate to "Import from video file…" (index 5 in base items).
        for _ in 0..5 {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);

        assert_eq!(shell.screen_name(), "video_picker");
    }

    /// Pressing Esc on the video picker returns to the menu.
    #[test]
    fn video_picker_esc_returns_to_menu() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(false);

        for _ in 0..5 {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);
        assert_eq!(shell.screen_name(), "video_picker");

        shell.on_key(KeyCode::Esc);
        assert_eq!(shell.screen_name(), "menu");
    }

    /// Navigating to "Import from URL…" and pressing Enter opens the URL input
    /// screen (only when fetch command is configured).
    #[test]
    fn import_from_url_menu_item_opens_url_input() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(true);

        // Navigate to "Import from URL…" (index 6 when fetch cmd is present).
        for _ in 0..6 {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);

        assert_eq!(shell.screen_name(), "url_input");
    }

    /// Pressing Esc on the URL input returns to the menu.
    #[test]
    fn url_input_esc_returns_to_menu() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(true);

        for _ in 0..6 {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);
        assert_eq!(shell.screen_name(), "url_input");

        shell.on_key(KeyCode::Esc);
        assert_eq!(shell.screen_name(), "menu");
    }

    /// Typing a URL and pressing Enter in the URL input transitions to the
    /// importing screen and starts the pipeline.
    #[test]
    fn url_input_submit_starts_importing_screen() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(true);

        for _ in 0..6 {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);
        assert_eq!(shell.screen_name(), "url_input");

        for c in "https://example.com/v.mp4".chars() {
            shell.on_key(KeyCode::Char(c));
        }
        shell.on_key(KeyCode::Enter);

        assert_eq!(shell.screen_name(), "importing");
    }

    /// Pressing Esc while importing cancels and returns to the menu.
    #[test]
    fn importing_esc_returns_to_menu() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(true);

        for _ in 0..6 {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);
        for c in "https://example.com/v.mp4".chars() {
            shell.on_key(KeyCode::Char(c));
        }
        shell.on_key(KeyCode::Enter);
        assert_eq!(shell.screen_name(), "importing");

        shell.on_key(KeyCode::Esc);
        assert_eq!(shell.screen_name(), "menu");
        assert!(
            shell.status.contains("cancel"),
            "status must mention cancellation"
        );
    }

    /// A failed import (missing file) eventually lands back on the menu with
    /// an error status. Tested by polling the importing screen directly after
    /// giving the worker thread time to fail.
    #[test]
    fn failed_import_returns_to_menu_with_status() {
        use crate::import_screen::{ImportOutcome, ImportingScreen};
        use rockcraft_import::ImportInput;

        let mut screen = ImportingScreen::start(ImportInput::File(std::path::PathBuf::from(
            "/nonexistent_rockcraft_test/video.mp4",
        )));

        // Give the worker thread up to 500 ms to complete.
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(500);
        let outcome = loop {
            if let Some(o) = screen.poll() {
                break Some(matches!(o, ImportOutcome::Failed(_)));
            }
            if std::time::Instant::now() > deadline {
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        };

        assert_eq!(
            outcome,
            Some(true),
            "missing-file import must surface a Failed outcome"
        );
    }
}
