//! The app shell: owns the live MIDI connection and switches between screens
//! (Menu / Edit / Play). Each screen handles its own rendering; the shell
//! drains MIDI once per frame and routes events to the active screen.
//!
//! Capture and editing are a single screen (`Screen::Edit`): the editor both
//! records (StepRecord / LiveRecord input modes, toggled by `R` / `t`) and
//! hand-edits the same timeline (M9-A). There is no separate record screen.

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
use rockcraft_core::{Grid, Key, Mixer, RecordingMeta, Scale, SynthBus, Timeline, TrackOrigin};
use rockcraft_import::{fetch_command_configured, ImportInput};
use rockcraft_midi::{smf_bytes_to_events, NoteSource};
use tokio::sync::mpsc;

use crate::backing::{BackingPicker, PickerOutcome};
use crate::edit::{EditScreen, NameOutcome, PromptOutcome, SplitOutcome};
use crate::import_screen::{
    ImportOutcome, ImportingScreen, SourceKind, SourcePicker, SourcePickerOutcome, UrlInput,
    UrlInputOutcome,
};
use crate::key_source::{CrosstermKeys, KeySource};
use crate::library::{default_scan_roots, library_root};
use crate::library_screen::{LibraryOutcome, LibraryScreen};
use crate::play::PlayScreen;

// ---------------------------------------------------------------------------
// Shell
// ---------------------------------------------------------------------------

/// Which screen is active.
pub(crate) enum Screen {
    Menu,
    Play(Box<PlayScreen>),
    /// The unified capture + edit surface (M9-A): records (StepRecord /
    /// LiveRecord) and hand-edits the same timeline. There is no separate
    /// record screen — "New piece" arms this screen in record mode.
    Edit(Box<EditScreen>),
    /// Browse for a backing audio file. `return_to` carries the editor to
    /// resume after picking (M9-E): backing is now chosen *for the loaded piece*
    /// from inside the edit screen, not as a piece-less top-level action. The
    /// chosen file persists into the piece's `meta.backing` on the next save.
    BackingPicker {
        picker: BackingPicker,
        return_to: Box<EditScreen>,
    },
    /// Browse for a local video or score file to import.
    SourcePicker(SourcePicker),
    /// Text input for a video URL to import.
    UrlInput(UrlInput),
    /// Running the import pipeline, showing progress.
    Importing(ImportingScreen),
    /// Browse the track library and open a bundle in Play or Edit.
    Library(LibraryScreen),
}

/// Fixed menu entries always shown.
///
/// M9-A collapsed Record / Compose (new) / Edit last recording into the single
/// capture+edit screen. "New piece" opens it empty and armed for recording;
/// "Continue last" reopens the most recent bundle in the same screen.
// "Choose backing track" was relocated into the unified capture/edit screen
// (M9-E): backing now attaches to the loaded piece (the `B` key there) and
// persists into its bundle, instead of being a piece-less top-level action.
const MENU_ITEMS_BASE: &[&str] = &[
    "New piece",
    "Continue last",
    "Play last recording",
    "Import from video file…",
    "Import score or scan…",
    "Library…",
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
    /// Optional backing track path forwarded into a new capture+edit session.
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
    /// Sound selection + levels (M14-C). Shell-wide, so a change made between
    /// takes survives the screen the play session lives on: synth-bus settings
    /// go straight at the synth, and the backing level is carried onto each new
    /// play screen (and its lazily-armed backing sink).
    mixer: Mixer,
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
            mixer: Mixer::new(),
        }
    }

    /// Override the fetch-command capability flag — used in tests.
    pub fn set_has_fetch_cmd(&mut self, v: bool) {
        self.has_fetch_cmd = v;
    }

    /// Hand a freshly loaded play screen the shell-wide mix (M14-C).
    ///
    /// Only the backing fader needs carrying: the two synth buses live on the
    /// synth itself, which every screen shares, so their instrument and level
    /// are already in force. The backing sink, by contrast, is created per take
    /// by the screen that owns it.
    fn tuned(&self, mut play: PlayScreen) -> PlayScreen {
        play.set_backing_gain(self.mixer.backing_gain);
        play
    }

    /// The shell's current mix. The TUI has no mixer UI — it is driven over the
    /// control socket (M14-C's picker lives in the desktop app) — so this is
    /// the read path for tests and for anything that wants to display it.
    pub fn mixer(&self) -> &Mixer {
        &self.mixer
    }

    /// Apply one mixer change and push it at the audio it controls (M14-C).
    ///
    /// Synth-bus settings go straight at the synth (shared by every screen);
    /// the backing level goes at the live play screen, and is remembered here
    /// for the takes that follow. Returns the resulting mix.
    fn apply_mixer<F>(&mut self, change: F) -> Result<rockcraft_core::MixerReport, String>
    where
        F: FnOnce(&mut Mixer) -> Result<(), rockcraft_core::MixerError>,
    {
        change(&mut self.mixer).map_err(|e| e.to_string())?;
        if let Some(synth) = &self.synth {
            for &bus in SynthBus::all() {
                let settings = self.mixer.bus(bus);
                let handle = synth.for_bus(bus);
                handle.set_instrument(settings.instrument);
                handle.set_gain(settings.gain);
            }
        }
        if let Screen::Play(play) = &mut self.screen {
            play.set_backing_gain(self.mixer.backing_gain);
        }
        Ok(rockcraft_core::MixerReport::from(self.mixer))
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
        // A backing track chosen this session plays under the composer at offset
        // 0 (M5-E adds an adjustable offset).
        if let Some(path) = &self.backing_path {
            edit = edit.with_backing(path.clone(), 0);
        }
        self.screen = Screen::Edit(Box::new(edit));
    }

    /// Load the bundle whose MIDI is at `midi_path` into the composer for
    /// editing. Reuses the bundle's `meta.json` for grid/key/backing/origin and
    /// falls back to a session backing track when the bundle declares none. On a
    /// read/parse failure the status line is set and the screen is unchanged.
    ///
    /// Shared by "Edit last recording" and the Library browser's "open in Edit".
    fn open_edit_from_midi(&mut self, midi_path: &std::path::Path) {
        let bytes = match std::fs::read(midi_path) {
            Ok(b) => b,
            Err(e) => {
                self.status = format!("read failed: {e}");
                return;
            }
        };
        let events = match smf_bytes_to_events(&bytes) {
            Ok(ev) => ev,
            Err(e) => {
                self.status = format!("parse failed: {e}");
                return;
            }
        };
        let timeline = Timeline::from_events(&events);
        let bundle_dir = midi_path.parent().unwrap_or(midi_path);
        let (grid, key) = load_meta_grid_key(bundle_dir);
        let mut edit = EditScreen::from_timeline(timeline, grid);
        edit.set_key(key);
        // Preserve the bundle's own provenance; a bundle with no origin recorded
        // becomes `Edited` once reopened in the composer.
        edit.set_origin(load_meta_origin(bundle_dir).unwrap_or(TrackOrigin::Edited));
        if let Some(s) = &self.synth {
            edit.attach_synth(s.clone());
        }
        // Prefer a backing the bundle itself declares (path + offset from meta);
        // otherwise fall back to a track chosen this session.
        if let Some((bpath, start)) = load_meta_backing(bundle_dir) {
            edit = edit.with_backing(bpath, start);
        } else if let Some(path) = &self.backing_path {
            edit = edit.with_backing(path.clone(), 0);
        }
        // Carry the bundle's background video reference (if any) so splitting the
        // piece round-trips the backdrop into the part bundles (M10-D). The TUI
        // never decodes it.
        if let Some((vpath, file, offset)) = load_meta_video(bundle_dir) {
            edit = edit.with_video(vpath, file, offset);
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
            // New piece: the unified capture+edit screen, empty and armed for
            // recording (StepRecord) so a played note lands immediately (M9-A).
            "New piece" => {
                self.activate_edit();
                if let Screen::Edit(edit) = &mut self.screen {
                    edit.arm_record();
                }
            }
            // Continue last: reopen the most recent bundle in the same unified
            // screen (in edit mode); `R` toggles back into recording.
            "Continue last" => match latest_recording() {
                Some(path) => self.open_edit_from_midi(&path),
                None => self.status = "no recordings yet — record one first".into(),
            },
            "Play last recording" => match latest_recording() {
                Some(path) => match load_play_screen(&path, self.synth.clone()) {
                    Ok(p) => self.screen = Screen::Play(Box::new(self.tuned(p))),
                    Err(e) => self.status = e,
                },
                None => self.status = "no recordings yet — record one first".into(),
            },
            "Library…" => {
                self.screen = Screen::Library(LibraryScreen::new(&default_scan_roots()));
            }
            "Import from video file…" => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                self.screen = Screen::SourcePicker(SourcePicker::new(cwd, SourceKind::Video));
            }
            "Import score or scan…" => {
                let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                self.screen = Screen::SourcePicker(SourcePicker::new(cwd, SourceKind::Score));
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
            // Enter/arrows) and the screen's own letter controls are handled
            // first; while record is armed, any unhandled key is forwarded to
            // the source as a note key — the mock maps the number row 1-0 to a
            // C-major scale and ignores the rest; a real piano ignores the
            // keyboard entirely. This routing now lives on the unified Edit
            // screen (M9-A); recording is its `R`-armed input mode.
            //
            // `r`/`m` precedence on Play: reserved controls; the mock
            // turns the number row into note presses (other keys are no-ops).
            Screen::Play(play) => match code {
                KeyCode::Tab | KeyCode::Esc => {
                    play.leave();
                    self.screen = Screen::Menu;
                }
                KeyCode::Char('r') => play.restart(),
                KeyCode::Char(' ') => play.toggle_pause(),
                KeyCode::Char('m') => play.toggle_hear_song(),
                KeyCode::Char('w') => play.toggle_wait_mode(),
                KeyCode::Char(c) => {
                    self.input.forward_key(c);
                }
                _ => {}
            },
            // The composer editor: Tab/Esc leave to the menu; `s` saves the
            // timeline bundle (matching Record's convention); every other key is
            // routed into the screen's own keymap. Exception: while the chord
            // selector is active, Esc cancels the chord instead of leaving.
            //
            // Note-entry vs command-entry (#124): the mock keyboard's note keys
            // are the *number row* (a no-op on a real piano), while editor
            // commands are letters/symbols — disjoint sets. We only forward a key
            // as a note while record is armed, so an unarmed `0` stays the
            // "cursor to lowest pitch" command. `forward_key` returns `false` for
            // unmapped keys (and on a real piano), so those fall through to the
            // editor keymap unchanged.
            //
            // When the editor is dirty and Tab/Esc is pressed, an exit-prompt
            // overlay is shown first ("Save / Discard / Cancel").
            Screen::Edit(edit) => {
                if edit.is_naming() {
                    // The "save to library" name overlay owns all keys while up.
                    match edit.on_name_key(code) {
                        NameOutcome::Submitted(name) => {
                            match edit.save_to_library(&library_root(), &name) {
                                Ok(p) => {
                                    let n = p
                                        .file_name()
                                        .map(|n| n.to_string_lossy().into_owned())
                                        .unwrap_or_default();
                                    edit.set_save_flash(format!("Saved {n} ✓"));
                                    edit.mark_clean();
                                    self.status = format!("saved {}", p.display());
                                }
                                Err(e) => self.status = format!("save failed: {e}"),
                            }
                        }
                        NameOutcome::Cancelled | NameOutcome::Pending => {}
                    }
                } else if edit.in_split_mode() {
                    // The split panel owns the keymap; only the actual write to
                    // the library comes back here (the shell owns the root +
                    // status line). Marker / segment edits are applied inside.
                    if let SplitOutcome::SaveParts = edit.on_split_key(code) {
                        match edit.split_into_library(&library_root()) {
                            Ok(dirs) => {
                                let n = dirs.len();
                                edit.set_save_flash(format!("Saved {n} part(s) ✓"));
                                let paths = dirs
                                    .iter()
                                    .map(|d| d.display().to_string())
                                    .collect::<Vec<_>>()
                                    .join(", ");
                                self.status = format!("saved {n} part(s): {paths}");
                            }
                            Err(e) => self.status = format!("split failed: {e}"),
                        }
                    }
                } else if edit.is_prompting_exit() {
                    // Route all keys to the prompt while it's visible.
                    match edit.on_prompt_key(code) {
                        PromptOutcome::SaveAndLeave => match edit.save() {
                            Ok(p) => {
                                self.status = format!("saved {}", p.display());
                                edit.leave();
                                self.screen = Screen::Menu;
                            }
                            Err(e) => {
                                // Save failed: report it and dismiss the prompt so the
                                // user can fix things before trying again.
                                self.status = format!("save failed: {e}");
                                edit.dismiss_exit_prompt();
                            }
                        },
                        PromptOutcome::Leave => {
                            edit.leave();
                            self.screen = Screen::Menu;
                        }
                        PromptOutcome::Stay => {
                            edit.dismiss_exit_prompt();
                        }
                    }
                } else {
                    match code {
                        KeyCode::Esc if edit.in_chord_mode() => edit.on_key(KeyCode::Esc),
                        // Clear visual selection on Esc without leaving the editor.
                        KeyCode::Esc if edit.in_visual_mode() => edit.on_key(KeyCode::Esc),
                        // Dirty editor: show the Save/Discard/Cancel prompt.
                        KeyCode::Tab | KeyCode::Esc if edit.is_dirty() => {
                            edit.start_exit_prompt();
                        }
                        KeyCode::Tab | KeyCode::Esc => {
                            edit.leave();
                            self.screen = Screen::Menu;
                        }
                        KeyCode::Char('s') => match edit.save() {
                            Ok(p) => {
                                let name = p
                                    .file_name()
                                    .map(|n| n.to_string_lossy().into_owned())
                                    .unwrap_or_default();
                                edit.set_save_flash(format!("Saved {name} ✓"));
                                edit.mark_clean();
                                self.status = format!("saved {}", p.display());
                            }
                            Err(e) => self.status = format!("save failed: {e}"),
                        },
                        // Shift-S opens the "save to library" name overlay.
                        KeyCode::Char('S') => edit.start_save_prompt(),
                        // `X` opens the split panel (drop markers, keep/discard
                        // and name segments, save kept parts — M10-D).
                        KeyCode::Char('X') => edit.enter_split_mode(),
                        // `B` opens the backing-track picker *for the loaded
                        // piece* (M9-E). Stop the editor's audition/backing,
                        // then stash the editor in the picker so the choice
                        // returns to it (and persists into the bundle on save).
                        KeyCode::Char('B') => {
                            edit.leave();
                            let cwd =
                                std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
                            if let Screen::Edit(e) =
                                std::mem::replace(&mut self.screen, Screen::Menu)
                            {
                                self.screen = Screen::BackingPicker {
                                    picker: BackingPicker::new(cwd),
                                    return_to: e,
                                };
                            }
                        }
                        KeyCode::Char(c) if edit.is_recording() && self.input.forward_key(c) => {}
                        other => edit.on_key(other),
                    }
                }
            }
            Screen::BackingPicker { picker, .. } => {
                let outcome = picker.on_key(code).clone();
                match outcome {
                    // Attach the chosen file to the piece being edited and resume
                    // editing — saving the piece persists it into meta.backing
                    // (M9-E). The editor is taken back out of the picker variant.
                    PickerOutcome::Selected(path) => {
                        self.status = format!("backing: {}", path.display());
                        if let Screen::BackingPicker { return_to, .. } =
                            std::mem::replace(&mut self.screen, Screen::Menu)
                        {
                            let mut edit = return_to;
                            edit.set_backing(path);
                            self.screen = Screen::Edit(edit);
                        }
                    }
                    // Cancel: resume editing unchanged.
                    PickerOutcome::Cancelled => {
                        if let Screen::BackingPicker { return_to, .. } =
                            std::mem::replace(&mut self.screen, Screen::Menu)
                        {
                            self.screen = Screen::Edit(return_to);
                        }
                    }
                    PickerOutcome::Pending => {}
                }
            }
            Screen::SourcePicker(picker) => {
                let kind = picker.kind();
                let outcome = picker.on_key(code).clone();
                match outcome {
                    SourcePickerOutcome::Selected(path) => {
                        self.screen =
                            Screen::Importing(ImportingScreen::start(kind.input_for(path)));
                    }
                    SourcePickerOutcome::Cancelled => {
                        self.screen = Screen::Menu;
                    }
                    SourcePickerOutcome::Pending => {}
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
            Screen::Importing(imp) => {
                // Esc cancels (the thread continues running but we abandon it).
                // After a failure the screen lingers to show the output tail;
                // Esc there just dismisses, keeping the existing failure status.
                if code == KeyCode::Esc {
                    if !imp.has_failed() {
                        self.status = "import cancelled".into();
                    }
                    self.screen = Screen::Menu;
                }
            }
            Screen::Library(lib) => {
                let outcome = lib.on_key(code).clone();
                match outcome {
                    LibraryOutcome::OpenPlay(dir) => {
                        let midi = dir.join("song.mid");
                        match load_play_screen(&midi, self.synth.clone()) {
                            Ok(p) => self.screen = Screen::Play(Box::new(self.tuned(p))),
                            Err(e) => {
                                self.status = e;
                                self.screen = Screen::Menu;
                            }
                        }
                    }
                    LibraryOutcome::OpenEdit(dir) => {
                        self.open_edit_from_midi(&dir.join("song.mid"));
                    }
                    LibraryOutcome::Cancelled => {
                        self.screen = Screen::Menu;
                    }
                    LibraryOutcome::Pending => {}
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
        // Host commands (the app-level I/O tier) are shell-wide, not editor-only:
        // they drive play/library/etc. across screens. Dispatch them through the
        // shell's `HostServices` regardless of the active screen.
        if let Request::RunHostCommand {
            id,
            command,
            params,
        } = &req
        {
            return rockcraft_control::handle_run_host_command(self, *id, command, params);
        }
        // `query help` returns the full catalog (actions + host commands) from
        // anywhere — agents discover the surface without first opening the editor.
        if let Request::Query {
            id,
            what: QueryKind::Help,
        } = &req
        {
            let mut composer = rockcraft_core::Composer::new();
            return rockcraft_control::handle(
                &mut composer,
                Request::Query {
                    id: *id,
                    what: QueryKind::Help,
                },
            );
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
            Screen::Play(_) => "play",
            Screen::Edit(_) => "edit",
            Screen::BackingPicker { .. } => "backing_picker",
            Screen::SourcePicker(p) => match p.kind() {
                SourceKind::Video => "video_picker",
                SourceKind::Score => "score_picker",
            },
            Screen::UrlInput(_) => "url_input",
            Screen::Importing(_) => "importing",
            Screen::Library(_) => "library",
        }
    }

    /// Whether the shell has been asked to quit.
    pub fn is_quit(&self) -> bool {
        self.should_quit
    }
}

// ---------------------------------------------------------------------------
// Host-command tier (app-level workflows over the control protocol)
// ---------------------------------------------------------------------------

/// The TUI's app-level command surface (M8-A).
///
/// `core::Action`s are handled by the editor's composer; these are the I/O
/// workflows (library/play/...) the keyboard drives through the menu and library
/// screens. The single exhaustive `match` is the compiler-enforced seam: a new
/// [`HostCommand`](rockcraft_control::HostCommand) variant won't compile until
/// this arm exists.
///
/// The TUI's record / import / backing flows are interactive screen state
/// machines (text prompts, file pickers) with no headless entry point, so they
/// return [`HostError::Unsupported`] here — an explicit, still-exhaustive arm.
/// The library / play workflows, which *do* have direct entry points, are wired.
impl rockcraft_control::HostServices for Shell {
    fn dispatch(
        &mut self,
        cmd: rockcraft_control::HostCommand,
    ) -> Result<serde_json::Value, rockcraft_control::HostError> {
        use rockcraft_control::{HostCommand, HostError};
        use serde_json::json;

        match cmd {
            HostCommand::ScanLibrary => {
                let entries: Vec<serde_json::Value> =
                    rockcraft_midi::bundle::list_library(&default_scan_roots())
                        .into_iter()
                        .map(|e| {
                            json!({
                                "name": e.name,
                                "dir": e.dir.to_string_lossy(),
                                "note_count": e.note_count,
                                "duration_us": e.duration_us,
                                "has_backing": e.has_backing,
                            })
                        })
                        .collect();
                Ok(json!(entries))
            }
            HostCommand::QueryDirty => {
                // Only the editor tracks unsaved edits; other screens are clean.
                let dirty = matches!(&self.screen, Screen::Edit(edit) if edit.is_dirty());
                Ok(json!(dirty))
            }
            HostCommand::PlayLoad { dir } => {
                let midi = std::path::Path::new(&dir).join("song.mid");
                match load_play_screen(&midi, self.synth.clone()) {
                    Ok(play) => {
                        self.screen = Screen::Play(Box::new(self.tuned(play)));
                        Ok(json!({ "loaded": dir }))
                    }
                    Err(detail) => Err(HostError::Failed {
                        command: "play_load".into(),
                        detail,
                    }),
                }
            }
            // Interactive, screen-state-machine workflows with no headless entry
            // point in the TUI. Explicit, compiler-checked Unsupported arms.
            HostCommand::SaveBundle { .. } => Err(HostError::Unsupported("save_bundle".into())),
            HostCommand::LoadBundle { .. } => Err(HostError::Unsupported("load_bundle".into())),
            HostCommand::SplitBundle { .. } => Err(HostError::Unsupported("split_bundle".into())),
            HostCommand::PlaySetWait { .. } => Err(HostError::Unsupported("play_set_wait".into())),
            HostCommand::PlayToggleHearSong => {
                Err(HostError::Unsupported("play_toggle_hear_song".into()))
            }
            // Pause/resume the live play session over the socket, mirroring the
            // `Space` key. A no-op error (not a panic) off the play screen.
            HostCommand::PlayTogglePause => {
                if let Screen::Play(play) = &mut self.screen {
                    play.toggle_pause();
                    Ok(json!({ "paused": play.is_paused() }))
                } else {
                    Err(HostError::Failed {
                        command: "play_toggle_pause".into(),
                        detail: "no active play session".into(),
                    })
                }
            }
            HostCommand::PlayFinish => Err(HostError::Unsupported("play_finish".into())),
            HostCommand::RecordStart { .. } => Err(HostError::Unsupported("record_start".into())),
            HostCommand::RecordStop => Err(HostError::Unsupported("record_stop".into())),
            HostCommand::RecordSave => Err(HostError::Unsupported("record_save".into())),
            HostCommand::AttachBacking { .. } => {
                Err(HostError::Unsupported("attach_backing".into()))
            }
            HostCommand::DetachBacking => Err(HostError::Unsupported("detach_backing".into())),
            // Sound selection + levels (M14-C). Shell-wide, so these work from
            // any screen — the synth is shared and the backing fader is carried
            // onto the next take.
            HostCommand::SetInstrument { bus, instrument } => self
                .apply_mixer(|m| m.set_instrument(bus, &instrument).map(|_| ()))
                .and_then(|report| serde_json::to_value(report).map_err(|e| e.to_string()))
                .map_err(|detail| HostError::Failed {
                    command: "set_instrument".into(),
                    detail,
                }),
            HostCommand::SetBusGain { bus, gain } => self
                .apply_mixer(|m| m.set_gain(bus, gain).map(|_| ()))
                .and_then(|report| serde_json::to_value(report).map_err(|e| e.to_string()))
                .map_err(|detail| HostError::Failed {
                    command: "set_bus_gain".into(),
                    detail,
                }),
            HostCommand::QueryMixer => {
                serde_json::to_value(rockcraft_core::MixerReport::from(self.mixer)).map_err(|e| {
                    HostError::Failed {
                        command: "query_mixer".into(),
                        detail: e.to_string(),
                    }
                })
            }
            HostCommand::AttachVideo { .. } => Err(HostError::Unsupported("attach_video".into())),
            HostCommand::SetVideoOffset { .. } => {
                Err(HostError::Unsupported("set_video_offset".into()))
            }
            HostCommand::DetachVideo => Err(HostError::Unsupported("detach_video".into())),
            HostCommand::QueryVideo => Err(HostError::Unsupported("query_video".into())),
            HostCommand::ImportStart { .. } => Err(HostError::Unsupported("import_start".into())),
            HostCommand::ImportScore { .. } => Err(HostError::Unsupported("import_score".into())),
            HostCommand::AudioStatus => Err(HostError::Unsupported("audio_status".into())),
            HostCommand::MidiStatus => Err(HostError::Unsupported("midi_status".into())),
            HostCommand::RecordStatus => Err(HostError::Unsupported("record_status".into())),
            HostCommand::AppQuit => Err(HostError::Unsupported("app_quit".into())),
        }
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
                Screen::Play(play) => play.ingest(ev),
                // The unified capture+edit screen (M9-A): in a record input mode
                // the editor consumes played notes (step / live record); in
                // direct-edit `ingest` ignores them for placement. Either way we
                // sound the keys you play.
                Screen::Edit(edit) => {
                    edit.ingest(ev);
                    if let Some(s) = &synth {
                        s.apply(&ev);
                    }
                }
                // These screens ignore live MIDI input.
                Screen::Menu
                | Screen::BackingPicker { .. }
                | Screen::SourcePicker(_)
                | Screen::UrlInput(_)
                | Screen::Importing(_)
                | Screen::Library(_) => {}
            }
        }

        // Tick song-synth triggers and the backing track (clock-driven, not
        // frame-rate-driven); the backing arms itself once the clock reaches the
        // lead-in's end so it lines up with the notes hitting the keyboard line.
        if let Screen::Play(play) = &mut shell.screen {
            // Advance the pausable clock first (wait-mode may freeze it), then
            // fire synth/backing for the resulting clock position.
            play.tick();
            play.tick_song_synth();
            play.tick_backing();
        }

        // Tick editor transport audition and the backing track (clock-driven);
        // the backing arms on transport play and re-syncs on stop/seek/loop-wrap.
        if let Screen::Edit(edit) = &mut shell.screen {
            edit.tick_audition();
            edit.tick_backing();
        }

        // A finished song returns to the menu on its own.
        if let Screen::Play(play) = &shell.screen {
            if play.is_finished() {
                shell.status = "song finished".into();
                shell.screen = Screen::Menu;
            }
        }

        // Poll the import pipeline and handle completion.
        apply_import_outcome(shell);

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
        Screen::Play(play) => play.draw(f, f.area()),
        Screen::Edit(edit) => edit.draw(f, f.area()),
        Screen::BackingPicker { picker, .. } => picker.draw(f, f.area()),
        Screen::SourcePicker(picker) => picker.draw(f, f.area()),
        Screen::UrlInput(ui) => ui.draw(f, f.area()),
        Screen::Importing(imp) => imp.draw(f, f.area()),
        Screen::Library(lib) => lib.draw(f, f.area()),
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

/// Resolve a bundle's backing track from its `meta.json`, returning the
/// absolute file path (relative to the bundle dir, so it stays movable) and the
/// `audio_start_us` offset. `None` when there is no manifest or no backing.
fn load_meta_backing(bundle_dir: &std::path::Path) -> Option<(PathBuf, u64)> {
    let json = std::fs::read_to_string(bundle_dir.join("meta.json")).ok()?;
    let meta = RecordingMeta::from_json(&json).ok()?;
    let backing = meta.backing?;
    Some((bundle_dir.join(&backing.file), backing.audio_start_us))
}

/// Read a bundle's recorded provenance from its `meta.json`; `None` when there
/// is no manifest, it fails to parse, or it predates the `origin` field.
fn load_meta_origin(bundle_dir: &std::path::Path) -> Option<TrackOrigin> {
    let json = std::fs::read_to_string(bundle_dir.join("meta.json")).ok()?;
    RecordingMeta::from_json(&json).ok()?.origin
}

/// Resolve a bundle's background video from its `meta.json`, returning the
/// absolute file path (relative to the bundle dir, so it stays movable), the
/// bundle-relative filename, and the `offset_us`. `None` when there is no
/// manifest or no video. The TUI never decodes the file — this is only so a
/// split round-trips the backdrop reference (M10-D).
fn load_meta_video(bundle_dir: &std::path::Path) -> Option<(PathBuf, String, i64)> {
    let json = std::fs::read_to_string(bundle_dir.join("meta.json")).ok()?;
    let meta = RecordingMeta::from_json(&json).ok()?;
    let video = meta.video?;
    Some((bundle_dir.join(&video.file), video.file, video.offset_us))
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

/// Poll a running import and act on its outcome; a no-op on every other screen.
///
/// Success loads the finished bundle into Play through the same
/// [`load_play_screen`] the library path uses, so an imported piece and the same
/// bundle re-opened later behave identically (#247). Failure leaves the
/// Importing screen up so its output tail stays readable next to the error.
///
/// Split out of `run_loop` so the completion branch is reachable headlessly.
fn apply_import_outcome(shell: &mut Shell) {
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
                    shell.screen = Screen::Play(Box::new(shell.tuned(play)));
                }
                Err(e) => {
                    shell.status = format!("import succeeded but load failed: {e}");
                    shell.screen = Screen::Menu;
                }
            }
        }
        Some(Err(msg)) => {
            // Status carries only the first line (the rest is shown in the pane).
            let first = msg.lines().next().unwrap_or(&msg);
            shell.status = format!("import failed: {first}");
        }
        None => {}
    }
}

/// Build a [`PlayScreen`] for the MIDI at `midi_path`, attaching the bundle's
/// backing track when its sibling `meta.json` declares one. Loose `.mid` files
/// (no sibling manifest) load MIDI-only, exactly as before.
///
/// "Hear the song" defaults to on exactly when the piece has no backing track
/// (#247): a MIDI-only bundle would otherwise open silent unless a live piano is
/// connected, while a piece with a real recording behind it doesn't need the
/// synth doubling the melody. The `m` key still toggles it either way.
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
    let mut has_backing = false;
    if let Some(dir) = midi_path.parent() {
        if let Ok(json) = std::fs::read_to_string(dir.join("meta.json")) {
            if let Ok(meta) = RecordingMeta::from_json(&json) {
                if let Some(backing) = meta.backing {
                    play = play.with_backing(dir.join(&backing.file), backing.audio_start_us);
                    has_backing = true;
                }
            }
        }
    }
    Ok(play.with_hear_song(!has_backing))
}

#[cfg(test)]
mod tests {
    use super::*;
    use rockcraft_control::{HostCommand, HostServices, SaveDest};
    use rockcraft_core::{Grid, Key, MidiNote, Note, Scale, Timeline, Velocity};
    use rockcraft_midi::ScriptedSource;
    use tokio::sync::oneshot;

    /// A sample of every [`HostCommand`] variant, for the no-panic dispatch test.
    fn every_host_command() -> Vec<HostCommand> {
        vec![
            HostCommand::ScanLibrary,
            HostCommand::SaveBundle {
                dest: SaveDest::QuickSave,
            },
            HostCommand::LoadBundle { dir: "x".into() },
            HostCommand::QueryDirty,
            HostCommand::SplitBundle { segments: vec![] },
            HostCommand::PlayLoad {
                dir: "does/not/exist".into(),
            },
            HostCommand::PlaySetWait { on: true },
            HostCommand::PlayToggleHearSong,
            HostCommand::PlayTogglePause,
            HostCommand::PlayFinish,
            HostCommand::RecordStart { backing: None },
            HostCommand::RecordStop,
            HostCommand::RecordSave,
            HostCommand::AttachBacking {
                path: "b.ogg".into(),
            },
            HostCommand::DetachBacking,
            HostCommand::AttachVideo {
                path: "movie.mp4".into(),
                offset_us: -100_000,
            },
            HostCommand::SetVideoOffset { offset_us: 0 },
            HostCommand::DetachVideo,
            HostCommand::QueryVideo,
            HostCommand::ImportStart { url: "u".into() },
            HostCommand::ImportScore {
                path: "s.musicxml".into(),
            },
            HostCommand::SetInstrument {
                bus: rockcraft_core::SynthBus::Song,
                instrument: "marimba".into(),
            },
            HostCommand::SetBusGain {
                bus: rockcraft_core::MixerBus::Backing,
                gain: 0.5,
            },
            HostCommand::QueryMixer,
            HostCommand::AudioStatus,
            HostCommand::MidiStatus,
            HostCommand::RecordStatus,
            HostCommand::AppQuit,
        ]
    }

    /// Every host command dispatches without panicking. ScanLibrary and
    /// QueryDirty succeed; PlayLoad on a missing dir fails cleanly; the
    /// interactive workflows return Unsupported. None of them panic.
    #[test]
    fn shell_host_dispatch_handles_every_command() {
        for cmd in every_host_command() {
            let mut shell = make_shell();
            let name = cmd.name();
            // The result is allowed to be Ok or Err — what matters is no panic.
            let _ = shell.dispatch(cmd);
            // ScanLibrary and QueryDirty must always succeed.
            if name == "scan_library" {
                let mut shell = make_shell();
                assert!(shell.dispatch(HostCommand::ScanLibrary).is_ok());
            }
            if name == "query_dirty" {
                let mut shell = make_shell();
                assert_eq!(
                    shell.dispatch(HostCommand::QueryDirty).unwrap(),
                    serde_json::json!(false),
                    "a fresh shell (menu screen) is not dirty"
                );
            }
        }
    }

    /// The mixer commands work from the menu screen — the synth is shell-wide,
    /// not owned by a session — and each answers with the whole new mix.
    #[test]
    fn mixer_commands_work_off_the_play_screen() {
        use rockcraft_core::{MixerBus, SynthBus};

        let mut shell = make_shell();
        assert_eq!(shell.screen_name(), "menu");

        let report = shell
            .dispatch(HostCommand::SetInstrument {
                bus: SynthBus::Player,
                instrument: "vibraphone".into(),
            })
            .expect("set_instrument from the menu");
        assert_eq!(report["player"]["instrument"]["id"], "vibraphone");
        assert_eq!(report["song"]["instrument"]["id"], "grand_piano");

        let report = shell
            .dispatch(HostCommand::SetBusGain {
                bus: MixerBus::Song,
                gain: 0.25,
            })
            .expect("set_bus_gain");
        assert_eq!(report["song"]["gain"], 0.25);
        assert_eq!(report["player"]["gain"], 1.0, "one fader at a time");

        // The catalog rides along so a client never hardcodes the list.
        let mixer = shell
            .dispatch(HostCommand::QueryMixer)
            .expect("query_mixer");
        assert_eq!(mixer["player"]["instrument"]["id"], "vibraphone");
        assert_eq!(mixer["song"]["gain"], 0.25);
        assert_eq!(
            mixer["instruments"].as_array().unwrap().len(),
            rockcraft_core::instruments().len()
        );
    }

    /// A bad instrument id / gain is reported as a failed command and leaves
    /// the mix as it was.
    #[test]
    fn mixer_commands_reject_bad_input() {
        use rockcraft_core::{MixerBus, SynthBus};

        let mut shell = make_shell();
        let err = shell
            .dispatch(HostCommand::SetInstrument {
                bus: SynthBus::Player,
                instrument: "kazoo".into(),
            })
            .unwrap_err();
        assert!(matches!(err, rockcraft_control::HostError::Failed { .. }));
        assert!(shell
            .dispatch(HostCommand::SetBusGain {
                bus: MixerBus::Player,
                gain: f32::NAN,
            })
            .is_err());
        assert_eq!(
            shell.mixer().player.instrument.id,
            rockcraft_core::DEFAULT_INSTRUMENT
        );
        assert_eq!(shell.mixer().player.gain, rockcraft_core::Gain::UNITY);
    }

    /// The backing fader set between takes reaches the play screen opened
    /// after it — the mix is shell state, not per-session state.
    #[test]
    fn backing_gain_carries_onto_a_new_play_screen() {
        use rockcraft_core::{Gain, MixerBus};

        let mut shell = make_shell();
        shell
            .dispatch(HostCommand::SetBusGain {
                bus: MixerBus::Backing,
                gain: 0.5,
            })
            .expect("set backing gain");
        let play = load_play_screen(&midi_only_fixture().join("song.mid"), None)
            .expect("load MIDI-only bundle");
        shell.screen = Screen::Play(Box::new(shell.tuned(play)));
        assert_eq!(
            play_screen(&shell).backing_gain(),
            Gain::new(0.5).unwrap(),
            "the new take opens at the level the mixer is set to"
        );
    }

    /// The TUI drives import through its interactive screens, not the socket,
    /// so `import_score` is an explicit `Unsupported` — matching `import_start`.
    #[test]
    fn import_score_is_unsupported_in_the_tui() {
        let mut shell = make_shell();
        assert_eq!(
            shell
                .dispatch(HostCommand::ImportScore {
                    path: "s.musicxml".into(),
                })
                .unwrap_err(),
            rockcraft_control::HostError::Unsupported("import_score".into())
        );
    }

    /// `play_toggle_pause` off the play screen is a clean no-op error, not a
    /// panic (there is no session to freeze).
    #[test]
    fn play_toggle_pause_off_play_screen_is_a_clean_error() {
        let mut shell = make_shell();
        assert_eq!(shell.screen_name(), "menu");
        let err = shell.dispatch(HostCommand::PlayTogglePause).unwrap_err();
        match err {
            rockcraft_control::HostError::Failed { command, .. } => {
                assert_eq!(command, "play_toggle_pause");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
    }

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

    /// "New piece" (index 0) enters the unified capture+edit screen with an
    /// empty timeline, already armed for recording (M9-A), driven by the same
    /// key routing used in the live shell.
    #[test]
    fn new_piece_enters_empty_armed_edit_screen() {
        let mut shell = make_shell();

        shell.on_key(KeyCode::Enter); // index 0 = New piece

        assert_eq!(shell.screen_name(), "edit");
        assert_eq!(
            shell.edit_note_count(),
            Some(0),
            "new piece starts with an empty timeline"
        );
        if let Screen::Edit(e) = &shell.screen {
            assert!(
                e.is_recording(),
                "New piece opens the unified screen already armed for recording"
            );
            assert_eq!(
                e.input_mode(),
                rockcraft_core::InputMode::StepRecord,
                "New piece arms step-record by default"
            );
        } else {
            panic!("expected the unified edit screen");
        }
    }

    /// The retired `record` route no longer exists: there is no menu item that
    /// lands on a `"record"` screen — "New piece" lands on the unified editor.
    #[test]
    fn no_standalone_record_route() {
        let mut shell = make_shell();
        // None of the menu items name the old Record / Compose / Edit entries.
        let items = shell.menu_items();
        for retired in ["Record", "Compose (new)", "Edit last recording"] {
            assert!(
                !items.contains(&retired),
                "retired menu item still present: {retired}"
            );
        }
        // The unified entries are present.
        assert!(items.contains(&"New piece"));
        assert!(items.contains(&"Continue last"));

        // Activating every menu item never yields a "record" screen.
        for idx in 0..items.len() {
            shell.menu_state.select(Some(idx));
            shell.menu_activate();
            assert_ne!(
                shell.screen_name(),
                "record",
                "menu item {idx} routed to a retired record screen"
            );
            // Reset to the menu for the next activation.
            shell.screen = Screen::Menu;
        }
    }

    /// M9-E: the standalone "Choose backing track" menu item is gone — backing
    /// is now chosen from inside the edit screen. No menu item lands on the
    /// backing picker.
    #[test]
    fn no_standalone_backing_menu_item() {
        let mut shell = make_shell();
        let items = shell.menu_items();
        assert!(
            !items.contains(&"Choose backing track"),
            "backing-track menu item should be relocated into the edit screen"
        );
        for idx in 0..items.len() {
            shell.menu_state.select(Some(idx));
            shell.menu_activate();
            assert_ne!(
                shell.screen_name(),
                "backing_picker",
                "menu item {idx} still routes to the standalone backing picker"
            );
            shell.screen = Screen::Menu;
        }
    }

    /// M9-E: `B` in the editor opens the backing picker *for the loaded piece*;
    /// cancelling (Esc) returns to the same editor with its notes intact, and
    /// the editor exposes the picker entry point (not the menu).
    #[test]
    fn edit_b_opens_backing_picker_and_returns() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a')); // one note
        assert_eq!(shell.edit_note_count(), Some(1));

        shell.on_key(KeyCode::Char('B'));
        assert_eq!(
            shell.screen_name(),
            "backing_picker",
            "B opens the backing picker from the editor"
        );

        // Esc cancels the picker → back to the editor, notes preserved.
        shell.on_key(KeyCode::Esc);
        assert_eq!(shell.screen_name(), "edit", "cancel returns to the editor");
        assert_eq!(
            shell.edit_note_count(),
            Some(1),
            "the edited piece survives the picker round-trip"
        );
    }

    /// "Continue last" reopens the most recent bundle in the same unified edit
    /// screen (mirrors the former "Edit last recording"), pre-populated.
    #[test]
    fn continue_last_opens_latest_bundle_in_edit() {
        let mut tl = Timeline::new();
        tl.insert(make_note(60, 0, 500_000));
        tl.insert(make_note(64, 500_000, 500_000));
        let expected = tl.len();

        let base = std::env::temp_dir().join(format!(
            "rockcraft_continue_last_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let edit_src = EditScreen::from_timeline(tl, Grid::default_120());
        edit_src.save_bundle(&base).expect("seed save failed");

        // Drive the same shell path "Continue last" uses.
        let midi_path = latest_recording_from(&base).expect("recording not found");
        let mut shell = make_shell();
        shell.open_edit_from_midi(&midi_path);

        assert_eq!(shell.screen_name(), "edit");
        assert_eq!(
            shell.edit_note_count(),
            Some(expected),
            "Continue last lands on the unified editor with the saved notes"
        );

        std::fs::remove_dir_all(&base).ok();
    }

    /// Drain whatever the input source has queued into the active edit screen,
    /// mirroring the MIDI-routing the run loop does each frame.
    fn drain_input_into_edit(shell: &mut Shell) {
        let events = shell.input.events();
        if let Screen::Edit(edit) = &mut shell.screen {
            for ev in events {
                edit.ingest(ev);
            }
        }
    }

    /// The #124 fix: in the editor the mock note keys (the number row) only play
    /// notes while record is armed; unarmed, a digit stays an editor *command*
    /// and is never forwarded as a note.
    #[test]
    fn editor_forwards_digit_notes_only_when_record_armed() {
        use rockcraft_midi::MockKeyboard;

        let mut shell = Shell::new(Box::new(MockKeyboard::new()), None, None);
        shell.activate_edit();

        // Unarmed: `0` runs as the "cursor to lowest pitch" command, not a note.
        shell.on_key(KeyCode::Char('0'));
        drain_input_into_edit(&mut shell);
        assert_eq!(
            shell.edit_note_count(),
            Some(0),
            "no note is placed by a digit while in direct-edit"
        );
        if let Screen::Edit(e) = &shell.screen {
            assert_eq!(
                e.cursor().pitch,
                21,
                "unarmed `0` still moves the cursor to the lowest pitch (A0)"
            );
        } else {
            panic!("expected edit screen");
        }

        // Arm step-record, then play three digits: each forwards as a note.
        shell.on_key(KeyCode::Char('R'));
        shell.on_key(KeyCode::Char('1'));
        shell.on_key(KeyCode::Char('2'));
        shell.on_key(KeyCode::Char('3'));
        drain_input_into_edit(&mut shell);
        assert_eq!(
            shell.edit_note_count(),
            Some(3),
            "armed, the number row plays notes into the recorder"
        );

        // Letter commands still work while armed (they aren't note keys), so the
        // armed user keeps navigation/editing. `R` here disarms record.
        shell.on_key(KeyCode::Char('R'));
        if let Screen::Edit(e) = &shell.screen {
            assert!(!e.is_recording(), "`R` toggled record off even while armed");
        } else {
            panic!("expected edit screen");
        }
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

    // ── track library (issue #153) ──────────────────────────────────────────

    /// Save a chart into a temp library root, then browse it through the shell's
    /// Library screen and open it in Edit — the save→list→load round trip,
    /// driven via the same render/key machinery the live shell uses.
    #[test]
    fn library_save_list_load_round_trip() {
        use crate::library_screen::LibraryScreen;

        let mut tl = Timeline::new();
        tl.insert(make_note(60, 0, 500_000));
        tl.insert(make_note(64, 500_000, 500_000));
        let expected = tl.len();

        let root = std::env::temp_dir().join(format!(
            "rockcraft_lib_rt_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));

        // Save: a named bundle lands under the library root.
        let edit = EditScreen::from_timeline(tl, Grid::default_120());
        let dir = edit.save_to_library(&root, "My Song").expect("save");
        assert!(dir.join("song.mid").exists(), "bundle midi written");
        assert!(dir.join("meta.json").exists(), "bundle meta written");

        // List + render: the browser shows the saved track (snapshot machinery).
        let mut shell = make_shell();
        shell.screen = Screen::Library(LibraryScreen::new(std::slice::from_ref(&root)));
        let dump = shell.render_to_string(80, 24);
        assert!(
            dump.contains("my-song"),
            "library render must list the saved track, got:\n{dump}"
        );

        // Load: `e` opens the highlighted bundle in the editor, pre-populated.
        shell.on_key(KeyCode::Char('e'));
        assert_eq!(shell.screen_name(), "edit");
        assert_eq!(
            shell.edit_note_count(),
            Some(expected),
            "opened bundle carries its saved notes"
        );

        std::fs::remove_dir_all(&root).ok();
    }

    /// Opening a library entry in Play enters the Play screen.
    #[test]
    fn library_open_in_play() {
        use crate::library_screen::LibraryScreen;

        let mut tl = Timeline::new();
        tl.insert(make_note(60, 0, 500_000));
        let root = std::env::temp_dir().join(format!(
            "rockcraft_lib_play_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        let edit = EditScreen::from_timeline(tl, Grid::default_120());
        edit.save_to_library(&root, "play me").expect("save");

        let mut shell = make_shell();
        shell.screen = Screen::Library(LibraryScreen::new(std::slice::from_ref(&root)));
        shell.on_key(KeyCode::Enter); // Enter opens in Play
        assert_eq!(shell.screen_name(), "play");

        std::fs::remove_dir_all(&root).ok();
    }

    /// The menu lists "Library…" and Enter on it opens the Library browser.
    #[test]
    fn library_menu_item_opens_browser() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(false);
        let items = shell.menu_items();
        let idx = items
            .iter()
            .position(|s| *s == "Library…")
            .expect("Library menu item present");
        for _ in 0..idx {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);
        assert_eq!(shell.screen_name(), "library");
    }

    /// In the editor, Shift-S opens the save-to-library name overlay; typing a
    /// name and pressing Enter writes the bundle and clears the dirty flag.
    #[test]
    fn editor_shift_s_saves_to_library() {
        // Point the library root at a temp dir for the duration of this test.
        let root = std::env::temp_dir().join(format!(
            "rockcraft_lib_shiftS_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::env::set_var("ROCKCRAFT_LIBRARY_DIR", &root);

        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a')); // add a note → dirty

        shell.on_key(KeyCode::Char('S')); // open name overlay
        if let Screen::Edit(e) = &shell.screen {
            assert!(e.is_naming(), "Shift-S opens the name overlay");
        } else {
            panic!("expected edit screen");
        }
        for c in "demo".chars() {
            shell.on_key(KeyCode::Char(c));
        }
        shell.on_key(KeyCode::Enter); // submit → save

        if let Screen::Edit(e) = &shell.screen {
            assert!(!e.is_naming(), "overlay closes after save");
            assert!(!e.is_dirty(), "save clears the dirty flag");
        } else {
            panic!("expected edit screen");
        }
        assert!(
            root.join("demo").join("song.mid").exists(),
            "named bundle written under the library root"
        );

        std::env::remove_var("ROCKCRAFT_LIBRARY_DIR");
        std::fs::remove_dir_all(&root).ok();
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
            origin_us: 0,
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

        // Navigate to "Import from video file…" by name (order is M9-A's menu).
        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import from video file…")
            .expect("video import item present");
        for _ in 0..idx {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);

        assert_eq!(shell.screen_name(), "video_picker");
    }

    /// "Import score or scan…" is always offered (a score needs no fetch hook) and
    /// opens the score picker, not the video one.
    #[test]
    fn import_score_menu_item_opens_score_picker() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(false);

        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import score or scan…")
            .expect("score import item present without a fetch command");
        for _ in 0..idx {
            shell.on_key(KeyCode::Down);
        }
        shell.on_key(KeyCode::Enter);

        assert_eq!(shell.screen_name(), "score_picker");

        // Esc returns to the menu, like every other import sub-screen.
        shell.on_key(KeyCode::Esc);
        assert_eq!(shell.screen_name(), "menu");
    }

    /// Pressing Esc on the video picker returns to the menu.
    #[test]
    fn video_picker_esc_returns_to_menu() {
        let mut shell = make_shell();
        shell.set_has_fetch_cmd(false);

        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import from video file…")
            .expect("video import item present");
        for _ in 0..idx {
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
        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import from URL…")
            .expect("URL import item present");
        for _ in 0..idx {
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

        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import from URL…")
            .expect("URL import item present");
        for _ in 0..idx {
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

        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import from URL…")
            .expect("URL import item present");
        for _ in 0..idx {
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

        let idx = shell
            .menu_items()
            .iter()
            .position(|s| *s == "Import from URL…")
            .expect("URL import item present");
        for _ in 0..idx {
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

    // ── issue #127: save UX — feedback + dirty-exit prompt ──────────────────

    /// After pressing `s` in the editor the render shows the save confirmation
    /// and the dirty flag is cleared.
    #[test]
    fn save_key_shows_flash_and_clears_dirty() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a')); // add note → dirty

        if let Screen::Edit(edit) = &shell.screen {
            assert!(
                edit.is_dirty(),
                "editor should be dirty after adding a note"
            );
        }

        // Press `s` — this will write to the filesystem under recordings/.
        shell.on_key(KeyCode::Char('s'));

        let render = shell.render_to_string(80, 24);
        assert!(
            render.contains("Saved") && render.contains("✓"),
            "render must show save confirmation after `s`, got:\n{render}"
        );

        if let Screen::Edit(edit) = &shell.screen {
            assert!(!edit.is_dirty(), "dirty flag must be cleared after save");
        }
    }

    /// Pressing Esc on a clean (un-edited) editor exits immediately to the menu.
    #[test]
    fn clean_editor_esc_exits_immediately() {
        let mut shell = make_shell();
        shell.activate_edit();
        // No edits — editor is clean.
        shell.on_key(KeyCode::Esc);
        assert_eq!(shell.screen_name(), "menu", "clean editor exits on Esc");
    }

    /// Pressing Esc on a dirty editor shows the exit prompt (stays in editor).
    #[test]
    fn dirty_editor_esc_shows_exit_prompt() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a')); // dirty
        shell.on_key(KeyCode::Esc);
        assert_eq!(
            shell.screen_name(),
            "edit",
            "dirty editor must not leave on Esc"
        );
        let render = shell.render_to_string(80, 24);
        assert!(
            render.contains("Save") || render.contains("Discard"),
            "render must show exit prompt, got:\n{render}"
        );
    }

    /// Pressing Tab on a dirty editor also shows the exit prompt.
    #[test]
    fn dirty_editor_tab_shows_exit_prompt() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a'));
        shell.on_key(KeyCode::Tab);
        assert_eq!(shell.screen_name(), "edit");
        if let Screen::Edit(edit) = &shell.screen {
            assert!(edit.is_prompting_exit());
        }
    }

    /// On the exit prompt, `d` discards changes and leaves to the menu.
    #[test]
    fn exit_prompt_discard_leaves_to_menu() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a'));
        shell.on_key(KeyCode::Esc); // show prompt
        shell.on_key(KeyCode::Char('d')); // discard
        assert_eq!(shell.screen_name(), "menu");
    }

    /// On the exit prompt, any other key (treated as Cancel) dismisses the
    /// prompt and stays in the editor.
    #[test]
    fn exit_prompt_cancel_stays_in_editor() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a'));
        shell.on_key(KeyCode::Esc); // show prompt
        shell.on_key(KeyCode::Esc); // cancel (Esc on prompt → Stay)
        assert_eq!(shell.screen_name(), "edit");
        if let Screen::Edit(edit) = &shell.screen {
            assert!(!edit.is_prompting_exit(), "prompt must be dismissed");
        }
    }

    /// On the exit prompt, `s` saves and then leaves to the menu.
    #[test]
    fn exit_prompt_save_persists_and_leaves() {
        let mut shell = make_shell();
        shell.activate_edit();
        shell.on_key(KeyCode::Char('a'));
        shell.on_key(KeyCode::Esc); // show prompt
        shell.on_key(KeyCode::Char('s')); // save via prompt
        assert_eq!(
            shell.screen_name(),
            "menu",
            "save via prompt must leave to menu"
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

    // ── "hear the song" default (issue #247) ────────────────────────────────

    /// The committed fixture bundle: four notes, MIDI-only (its `meta.json`
    /// declares no backing track).
    fn midi_only_fixture() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("..")
            .join("fixtures")
            .join("play-bundle")
    }

    /// Copy the fixture's `song.mid` into a fresh temp bundle and write a
    /// `meta.json` that declares `backing` (or not).
    fn bundle_with_backing(tag: &str, backing: bool) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "rockcraft_hear_{tag}_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(midi_only_fixture().join("song.mid"), dir.join("song.mid")).unwrap();
        let meta = RecordingMeta {
            midi_file: "song.mid".into(),
            backing: backing.then(|| rockcraft_core::BackingTrack {
                file: "backing.ogg".into(),
                audio_start_us: 0,
            }),
            grid: None,
            key: None,
            origin: Some(TrackOrigin::Composed),
            video: None,
            version: 1,
        };
        std::fs::write(dir.join("meta.json"), meta.to_json()).unwrap();
        if backing {
            std::fs::write(dir.join("backing.ogg"), b"").unwrap();
        }
        dir
    }

    /// A MIDI-only piece would open silent without a live piano, so the synth
    /// audition defaults ON at load.
    #[test]
    fn hear_song_defaults_on_for_midi_only_bundle() {
        let play = load_play_screen(&midi_only_fixture().join("song.mid"), None)
            .expect("load fixture bundle");
        assert!(
            play.is_hear_song(),
            "a bundle with no backing track must audition itself"
        );
    }

    /// A piece with a real recording behind it doesn't need the synth doubling
    /// the melody, so the audition defaults OFF.
    #[test]
    fn hear_song_defaults_off_when_bundle_has_backing() {
        let dir = bundle_with_backing("backed", true);
        let play = load_play_screen(&dir.join("song.mid"), None).expect("load backed bundle");
        assert!(
            !play.is_hear_song(),
            "a bundle with a backing track must not audition over it"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The `m` toggle still flips in both directions from whichever default
    /// applied — the default only picks the starting side.
    #[test]
    fn m_toggles_hear_song_from_either_default() {
        let mut shell = make_shell();

        // MIDI-only: starts on, `m` turns it off, `m` again turns it back on.
        let play = load_play_screen(&midi_only_fixture().join("song.mid"), None).unwrap();
        shell.screen = Screen::Play(Box::new(play));
        shell.on_key(KeyCode::Char('m'));
        assert!(!play_screen(&shell).is_hear_song(), "m turns it off");
        shell.on_key(KeyCode::Char('m'));
        assert!(play_screen(&shell).is_hear_song(), "m turns it back on");

        // Backed: starts off, and `m` still lights it.
        let dir = bundle_with_backing("toggle", true);
        let play = load_play_screen(&dir.join("song.mid"), None).unwrap();
        shell.screen = Screen::Play(Box::new(play));
        assert!(!play_screen(&shell).is_hear_song(), "backed starts off");
        shell.on_key(KeyCode::Char('m'));
        assert!(play_screen(&shell).is_hear_song(), "m turns it on");
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn play_screen(shell: &Shell) -> &PlayScreen {
        match &shell.screen {
            Screen::Play(play) => play,
            _ => panic!("expected the play screen, got {}", shell.screen_name()),
        }
    }

    /// The regression the deleted import-only special case used to hide: a
    /// MIDI-only bundle opened straight off an import and the *same* bundle
    /// opened later from the library land in the same audition state.
    #[test]
    fn import_completion_and_library_load_agree_on_hear_song() {
        use crate::import_screen::{ImportingScreen, WorkerEvent};

        let bundle = midi_only_fixture();

        // Library path: load the bundle the way the browser does.
        let via_library = load_play_screen(&bundle.join("song.mid"), None).unwrap();

        // Import path: a finished worker hands the bundle dir to the shell.
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(WorkerEvent::Done(bundle.clone())).unwrap();
        drop(tx);
        let mut shell = make_shell();
        shell.screen = Screen::Importing(ImportingScreen::from_receiver(rx));
        apply_import_outcome(&mut shell);

        assert_eq!(shell.screen_name(), "play", "a done import opens Play");
        assert!(
            play_screen(&shell).is_hear_song(),
            "an imported MIDI-only piece must be audible"
        );
        assert_eq!(
            play_screen(&shell).is_hear_song(),
            via_library.is_hear_song(),
            "import and library loads must agree"
        );
    }
}
