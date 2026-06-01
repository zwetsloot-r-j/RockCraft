//! The ratatui view: a bottom keyboard that lights up held keys, plus a live
//! event log above it. Rendering and the input loop live here; the geometry and
//! state are in `keyboard.rs` (pure, tested).

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame, Terminal,
};
use rockcraft_core::{NoteEvent, NoteEventKind};
use rockcraft_midi::LiveInput;

use crate::keyboard::{
    black_key_col, is_black_key, white_key_slots, HeldNotes, Scale, HIGHEST_MIDI, LOWEST_MIDI,
};

/// How many recent events to keep in the scrolling log.
const LOG_CAPACITY: usize = 200;

/// Held-key highlight colour and the resting key colours.
const HELD_COLOR: Color = Color::Cyan;
const WHITE_KEY: Color = Color::White;
const BLACK_KEY: Color = Color::DarkGray;

/// Application state: everything the renderer needs.
pub struct App {
    held: HeldNotes,
    log: VecDeque<String>,
    port_name: String,
    should_quit: bool,
}

impl App {
    pub fn new(port_name: String) -> Self {
        Self {
            held: HeldNotes::new(),
            log: VecDeque::with_capacity(LOG_CAPACITY),
            port_name,
            should_quit: false,
        }
    }

    /// Fold one note event into the state (held set + log line).
    pub fn ingest(&mut self, ev: NoteEvent) {
        self.held.apply(&ev);
        let name = ev.note.name();
        let line = match ev.kind {
            NoteEventKind::On { velocity } if velocity.value() > 0 => {
                format!(
                    "{:>10}us  ON   {:<4} vel {}",
                    ev.timestamp_us,
                    name,
                    velocity.value()
                )
            }
            _ => format!("{:>10}us  OFF  {}", ev.timestamp_us, name),
        };
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }
}

/// Run the TUI against a connected `LiveInput` until the user quits (q / Esc).
pub fn run(mut input: LiveInput) -> io::Result<()> {
    let mut terminal = ratatui::init();
    let port = input.port_name().to_string();
    let mut app = App::new(port);

    let res = run_loop(&mut terminal, &mut app, &mut input);
    ratatui::restore();
    res
}

fn run_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
    input: &mut LiveInput,
) -> io::Result<()> {
    loop {
        // Drain everything the MIDI thread queued since last frame.
        for ev in input.events() {
            app.ingest(ev);
        }

        terminal.draw(|f| draw(f, app))?;

        // Poll keyboard at the frame cadence; rendering is decoupled from MIDI
        // timing (which is carried by NoteEvent::timestamp_us, not frames).
        if event::poll(Duration::from_millis(16))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press
                    && matches!(key.code, KeyCode::Char('q') | KeyCode::Esc)
                {
                    app.should_quit = true;
                }
            }
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn draw(f: &mut Frame, app: &App) {
    // Header (1) | event log (fill) | keyboard (4).
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(4),
    ])
    .split(f.area());

    draw_header(f, chunks[0], app);
    draw_log(f, chunks[1], app);
    draw_keyboard(f, chunks[2], app);
}

fn draw_header(f: &mut Frame, area: Rect, app: &App) {
    let held_text = if app.held.is_empty() {
        "—".to_string()
    } else {
        app.held
            .iter()
            .filter_map(|n| rockcraft_core::MidiNote::new(n).map(|m| m.name()))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", app.port_name),
            Style::default().fg(Color::Black).bg(HELD_COLOR),
        ),
        Span::raw(format!("  held: {:<2}  ", app.held.len())),
        Span::styled(held_text, Style::default().fg(HELD_COLOR)),
        Span::raw("   (q to quit)"),
    ]);
    f.render_widget(Paragraph::new(line), area);
}

fn draw_log(f: &mut Frame, area: Rect, app: &App) {
    // Show the most recent lines that fit.
    let inner_height = area.height.saturating_sub(2) as usize; // borders
    let lines: Vec<Line> = app
        .log
        .iter()
        .rev()
        .take(inner_height)
        .rev()
        .map(|s| Line::from(s.clone()))
        .collect();
    let block = Block::default().borders(Borders::ALL).title(" events ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_keyboard(f: &mut Frame, area: Rect, app: &App) {
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" keyboard (88) ");
    let inner = block.inner(area);
    f.render_widget(block, area);

    let scale = match Scale::best_for_width(inner.width) {
        Some(s) => s,
        None => {
            let msg = format!(
                "Widen terminal to ≥{} columns to show the keyboard",
                Scale::Compact.total_width()
            );
            f.render_widget(Paragraph::new(msg), inner);
            return;
        }
    };

    // Center the board within the available width.
    let board_w = scale.total_width();
    let x0 = inner.x + (inner.width.saturating_sub(board_w)) / 2;
    let body_y = inner.y;
    let body_h = inner.height;

    // 1) White keys as the base row(s).
    for slot in white_key_slots(scale) {
        let held = app.held.is_held(slot.note);
        let color = if held { HELD_COLOR } else { WHITE_KEY };
        let cell = "█".repeat(slot.width as usize);
        for dy in 0..body_h {
            let rect = Rect::new(x0 + slot.col, body_y + dy, slot.width, 1);
            f.render_widget(
                Paragraph::new(cell.clone()).style(Style::default().fg(color)),
                rect,
            );
        }
    }

    // 2) Black keys overlaid on the upper portion, between whites.
    let black_h = body_h.saturating_sub(1).max(1);
    for note in LOWEST_MIDI..=HIGHEST_MIDI {
        if !is_black_key(note) {
            continue;
        }
        let Some(col) = black_key_col(note, scale) else {
            continue;
        };
        let held = app.held.is_held(note);
        let color = if held { HELD_COLOR } else { BLACK_KEY };
        for dy in 0..black_h {
            let rect = Rect::new(x0 + col, body_y + dy, 1, 1);
            f.render_widget(Paragraph::new("▐").style(Style::default().fg(color)), rect);
        }
    }
}
