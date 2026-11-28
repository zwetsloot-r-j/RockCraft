//! End-to-end tests that drive the shell through its real `on_key` / run-loop
//! path using `ScriptedKeys` and ratatui's `TestBackend` — no terminal, no MIDI
//! hardware required.

use std::collections::VecDeque;

use crossterm::event::KeyCode;
use ratatui::backend::TestBackend;
use ratatui::Terminal;
use rockcraft_core::NoteEvent;
use rockcraft_midi::NoteSource;

use rockcraft_tui::app::{run_loop, Shell};
use rockcraft_tui::key_source::ScriptedKeys;

// ---------------------------------------------------------------------------
// Test helpers
// ---------------------------------------------------------------------------

/// A `NoteSource` that yields a pre-loaded sequence of events, then drains.
struct ScriptedNotes {
    events: VecDeque<NoteEvent>,
    port_name: String,
}

impl ScriptedNotes {
    fn empty() -> Self {
        Self {
            events: VecDeque::new(),
            port_name: "test".into(),
        }
    }
}

impl NoteSource for ScriptedNotes {
    fn events(&mut self) -> Vec<NoteEvent> {
        self.events.drain(..).collect()
    }

    fn port_name(&self) -> &str {
        &self.port_name
    }
}

fn make_shell(notes: ScriptedNotes) -> Shell {
    Shell::new(Box::new(notes), None, None)
}

fn make_terminal() -> Terminal<TestBackend> {
    Terminal::new(TestBackend::new(80, 24)).unwrap()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Pressing Enter opens the unified capture+edit screen ("New piece", index 0
/// is pre-selected); Tab returns to Menu (M9-A retired the separate record
/// route — index 0 now lands on the editor, armed).
#[test]
fn menu_navigation_enter_opens_new_piece_tab_returns_to_menu() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    // Menu starts at index 0 ("New piece"). Enter activates it.
    let mut keys = ScriptedKeys::new([
        KeyCode::Enter,     // activate "New piece" (already selected)
        KeyCode::Tab,       // return to Menu from the editor (clean → no prompt)
        KeyCode::Char('q'), // quit
    ]);

    assert_eq!(shell.screen_name(), "menu");
    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    // After the sequence the shell quit from Menu — screen is "menu".
    assert_eq!(shell.screen_name(), "menu");
}

/// Down arrow moves the menu cursor, Enter on "New piece" opens it, Esc returns.
#[test]
fn down_arrow_then_enter_then_esc_back_to_menu() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    // Start at index 0 (New piece). Move down to index 1 (Continue last), then
    // back up to 0, then Enter → unified edit screen.
    let mut keys = ScriptedKeys::new([
        KeyCode::Down,  // → index 1
        KeyCode::Up,    // → index 0 again
        KeyCode::Enter, // activate "New piece"
        KeyCode::Esc,   // return to Menu (clean editor → no prompt)
        KeyCode::Esc,   // quit (Esc from Menu == quit)
    ]);

    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    assert!(shell.is_quit());
    assert_eq!(shell.screen_name(), "menu");
}

/// `q` from the Menu triggers quit and the loop exits.
#[test]
fn q_from_menu_quits() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    let mut keys = ScriptedKeys::new([KeyCode::Char('q')]);
    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    assert!(shell.is_quit());
}

/// Esc from the Menu also triggers quit.
#[test]
fn esc_from_menu_quits() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    let mut keys = ScriptedKeys::new([KeyCode::Esc]);
    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    assert!(shell.is_quit());
}

/// Navigation keys (Tab/Esc) inside the unified editor return to Menu — not
/// forwarded as notes — the shell ends up back at Menu.
#[test]
fn tab_in_editor_is_navigation_not_note() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    let mut keys = ScriptedKeys::new([
        KeyCode::Enter,     // → unified edit screen (New piece)
        KeyCode::Tab,       // ← back to Menu (navigation, not a note)
        KeyCode::Char('q'), // quit
    ]);

    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    // Ended at Menu (Tab was navigation, not forwarded as a note).
    assert_eq!(shell.screen_name(), "menu");
}

/// Unrecognised keys inside the unified editor do not crash or change the
/// screen. ("New piece" arms step-record, so non-note keys still resolve as
/// editor commands or no-ops; the screen stays put until Tab.)
#[test]
fn unknown_keys_in_editor_are_ignored() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    let mut keys = ScriptedKeys::new([
        KeyCode::Enter,     // → unified edit screen (armed)
        KeyCode::Char('z'), // unmapped editor key (no-op)
        KeyCode::Char('q'), // 'q' is unmapped in the editor (only quits in Menu)
        KeyCode::Tab,       // ← Menu
        KeyCode::Char('q'), // quit from Menu
    ]);

    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    assert_eq!(shell.screen_name(), "menu");
}

/// "New piece" (menu index 0) opens the unified editor; Tab returns to the
/// Menu. Inside the editor, letter keys are navigation/commands (not forwarded
/// as notes) and don't escape the screen.
#[test]
fn new_piece_opens_editor_and_tab_returns_to_menu() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    let mut keys = ScriptedKeys::new([
        KeyCode::Enter,     // index 0 = New piece → open the editor
        KeyCode::Char('l'), // navigate (cursor right) — stays in editor
        KeyCode::Char('k'), // navigate (cursor up) — stays in editor
        KeyCode::Tab,       // ← back to Menu (clean editor → no prompt)
        KeyCode::Char('q'), // quit
    ]);

    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    assert!(shell.is_quit());
    assert_eq!(shell.screen_name(), "menu");
}

/// Vim-style `j`/`k` keys navigate the menu just like arrow keys.
#[test]
fn vim_keys_navigate_menu() {
    let mut shell = make_shell(ScriptedNotes::empty());
    let mut terminal = make_terminal();

    // j = Down, k = Up. Three j's wrap around from 0 → 1 → 2 → 0.
    let mut keys = ScriptedKeys::new([
        KeyCode::Char('j'), // index 1
        KeyCode::Char('j'), // index 2
        KeyCode::Char('j'), // index 0 (wrap)
        KeyCode::Char('k'), // index 2 (wrap the other way)
        KeyCode::Char('q'),
    ]);

    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();
    assert!(shell.is_quit());
}

// ---------------------------------------------------------------------------
// Piano menu navigation: C confirm · C# cancel · D up · E down
// ---------------------------------------------------------------------------

/// Yields one batch of note events per `events()` call, like a piano played
/// over several frames.
struct BatchedNotes {
    batches: VecDeque<Vec<NoteEvent>>,
}

impl NoteSource for BatchedNotes {
    fn events(&mut self) -> Vec<NoteEvent> {
        self.batches.pop_front().unwrap_or_default()
    }

    fn port_name(&self) -> &str {
        "test piano"
    }
}

/// Never presses a key — until a safety limit, so a broken wiring fails the
/// test instead of hanging it.
struct NoKeys {
    polls: u32,
}

const NO_KEYS_LIMIT: u32 = 10_000;

impl rockcraft_tui::key_source::KeySource for NoKeys {
    fn poll_key(&mut self, _timeout: std::time::Duration) -> std::io::Result<Option<KeyCode>> {
        self.polls += 1;
        Ok((self.polls >= NO_KEYS_LIMIT).then_some(KeyCode::Char('q')))
    }
}

fn press(note: u8, t: u64) -> Vec<NoteEvent> {
    use rockcraft_core::{MidiNote, Velocity};
    let n = MidiNote::new(note).unwrap();
    vec![
        NoteEvent::on(n, Velocity::new(90).unwrap(), t),
        NoteEvent::off(n, t + 100_000),
    ]
}

/// The whole menu is drivable from the piano alone: down into the video
/// importer, C# back out, C# again does *not* quit from the main menu, then up
/// (wrapping to "Quit") and C to confirm — in a low octave, to show any octave
/// works.
#[test]
fn piano_keys_navigate_menu_without_computer_keyboard() {
    const C: u8 = 36; // C2
    const CS: u8 = 37;
    const D: u8 = 38;
    const E: u8 = 40;
    let mut batches: VecDeque<Vec<NoteEvent>> = VecDeque::new();
    let mut t = 0;
    for note in [E, E, E, C, CS, CS, D, D, D, D, C] {
        batches.push_back(press(note, t));
        batches.push_back(Vec::new()); // an idle frame between presses
        t += 500_000;
    }
    let mut shell = Shell::new(Box::new(BatchedNotes { batches }), None, None);
    shell.set_has_fetch_cmd(false); // fixed menu: 6 items + Quit
    let mut terminal = make_terminal();
    let mut keys = NoKeys { polls: 0 };

    run_loop(&mut terminal, &mut shell, &mut keys).unwrap();

    assert!(
        keys.polls < NO_KEYS_LIMIT,
        "the piano's C on \"Quit\" should have ended the loop"
    );
    assert!(shell.is_quit());
    assert_eq!(shell.screen_name(), "menu");
}
