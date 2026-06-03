//! Injectable key-input abstraction for the run loop.
//!
//! `CrosstermKeys` wraps the crossterm event API and is the production default.
//! `ScriptedKeys` replays a fixed queue of key codes — used in end-to-end tests
//! to drive the shell through its real `on_key` path without a terminal.

use std::collections::VecDeque;
use std::io;
use std::time::Duration;

use crossterm::event::KeyCode;

/// A source of key-press events for the run loop.
///
/// Only key-DOWN presses are surfaced; key-up and repeat events are filtered
/// at the source. The loop calls `poll_key` once per frame and routes any
/// returned code through `Shell::on_key`.
pub trait KeySource {
    /// Return the next key press if one arrives within `timeout`, or `None` on
    /// timeout. An `Err` propagates as an I/O error that terminates the loop.
    fn poll_key(&mut self, timeout: Duration) -> io::Result<Option<KeyCode>>;
}

/// Production `KeySource`: forwards crossterm terminal events.
pub struct CrosstermKeys;

impl KeySource for CrosstermKeys {
    fn poll_key(&mut self, timeout: Duration) -> io::Result<Option<KeyCode>> {
        use crossterm::event::{self, Event, KeyEventKind};
        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    return Ok(Some(key.code));
                }
            }
        }
        Ok(None)
    }
}

/// Test `KeySource`: replays a pre-loaded queue of key codes.
///
/// `poll_key` pops and returns the front of the queue on every call, ignoring
/// `timeout`. Once drained it returns `None` on every subsequent call, letting
/// the run loop idle (render) until an explicit quit key in the sequence
/// terminates it.
pub struct ScriptedKeys {
    queue: VecDeque<KeyCode>,
}

impl ScriptedKeys {
    pub fn new(keys: impl IntoIterator<Item = KeyCode>) -> Self {
        Self {
            queue: keys.into_iter().collect(),
        }
    }
}

impl KeySource for ScriptedKeys {
    fn poll_key(&mut self, _timeout: Duration) -> io::Result<Option<KeyCode>> {
        Ok(self.queue.pop_front())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyCode;

    #[test]
    fn scripted_keys_returned_in_order() {
        let codes = vec![KeyCode::Down, KeyCode::Enter, KeyCode::Char('q')];
        let mut src = ScriptedKeys::new(codes.clone());
        for expected in &codes {
            assert_eq!(src.poll_key(Duration::ZERO).unwrap(), Some(*expected));
        }
    }

    #[test]
    fn scripted_keys_none_after_drain() {
        let mut src = ScriptedKeys::new([KeyCode::Up]);
        src.poll_key(Duration::ZERO).unwrap(); // consume
        assert_eq!(src.poll_key(Duration::ZERO).unwrap(), None);
        assert_eq!(src.poll_key(Duration::ZERO).unwrap(), None);
    }

    #[test]
    fn scripted_keys_empty_queue_returns_none_immediately() {
        let mut src = ScriptedKeys::new([]);
        assert_eq!(src.poll_key(Duration::ZERO).unwrap(), None);
    }
}
