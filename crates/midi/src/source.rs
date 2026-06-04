//! A swappable event source for frontends.
//!
//! The TUI (and later frontends) shouldn't name a concrete input type: they
//! want "a thing that yields [`NoteEvent`]s". [`NoteSource`] is that seam, so
//! the same run loop drives the real piano ([`crate::LiveInput`]) or a
//! piano-free [`crate::MockKeyboard`] without code changes.
//!
//! The trait lives in `midi`, not `core`: a source is bound to a wall clock and
//! a device/keyboard, which is exactly the kind of I/O `core` must stay free of
//! (see `CLAUDE.md`).

use rockcraft_core::NoteEvent;

/// Anything the frontend can pull note events from once per frame.
///
/// `events` is non-blocking and may return an empty `Vec`. `&mut self` is fine
/// because the frontend owns its source; a mock advances an internal clock/queue
/// on each call.
pub trait NoteSource {
    /// Note events received since the last call (non-blocking, may be empty).
    fn events(&mut self) -> Vec<NoteEvent>;
    /// Human-readable source name, shown in the menu header.
    fn port_name(&self) -> &str;

    /// Forward a typed computer-keyboard character to the source.
    ///
    /// A real piano ignores the keyboard and returns `false`; the
    /// [`MockKeyboard`](crate::MockKeyboard) maps a known key to a note (whose
    /// on/off pair then arrives via [`events`](NoteSource::events) on later
    /// frames) and returns `true`. The default is "not handled" so most sources
    /// need not implement it.
    fn forward_key(&mut self, _key: char) -> bool {
        false
    }
}

impl NoteSource for crate::LiveInput {
    fn events(&mut self) -> Vec<NoteEvent> {
        // Drain whatever the MIDI callback thread has queued. The existing
        // channel design is untouched — we just collect the non-blocking
        // iterator into the owned `Vec` the trait promises.
        crate::LiveInput::events(self).collect()
    }

    fn port_name(&self) -> &str {
        crate::LiveInput::port_name(self)
    }
}
