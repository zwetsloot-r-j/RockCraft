use crate::events::NoteEvent;

/// A chronologically-ordered, in-memory accumulator of [`NoteEvent`]s.
///
/// The buffer is the source of truth for a recording session. The MIDI crate
/// pushes events in; the TUI reads them out; the file serialiser drains them
/// to disk. Events are kept sorted by `timestamp_us`; an out-of-order insert
/// (rare in live input) is handled correctly via binary search.
pub struct EventBuffer {
    events: Vec<NoteEvent>,
}

impl EventBuffer {
    pub fn new() -> Self {
        Self { events: Vec::new() }
    }

    /// Push a [`NoteEvent`] into the buffer, maintaining timestamp order.
    ///
    /// Appending in order (the common live-input case) is O(1). An
    /// out-of-order event triggers a binary search + insert, O(n).
    pub fn push(&mut self, event: NoteEvent) {
        if self
            .events
            .last()
            .is_none_or(|last| last.timestamp_us <= event.timestamp_us)
        {
            self.events.push(event);
        } else {
            let pos = self
                .events
                .partition_point(|e| e.timestamp_us <= event.timestamp_us);
            self.events.insert(pos, event);
        }
    }

    /// All stored events in ascending timestamp order.
    pub fn events(&self) -> &[NoteEvent] {
        &self.events
    }

    /// Events whose `timestamp_us` falls within the closed interval
    /// `[start_us, end_us]`.
    pub fn events_in_range(&self, start_us: u64, end_us: u64) -> &[NoteEvent] {
        let lo = self.events.partition_point(|e| e.timestamp_us < start_us);
        let hi = self.events.partition_point(|e| e.timestamp_us <= end_us);
        &self.events[lo..hi]
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Remove all events, leaving the buffer empty.
    pub fn clear(&mut self) {
        self.events.clear();
    }

    /// Total duration of the recorded session: the timestamp of the last event,
    /// or 0 if the buffer is empty.
    pub fn duration_us(&self) -> u64 {
        self.events.last().map_or(0, |e| e.timestamp_us)
    }

    /// Drain all events out of the buffer in time order, leaving it empty.
    pub fn drain(&mut self) -> Vec<NoteEvent> {
        std::mem::take(&mut self.events)
    }
}

impl Default for EventBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MidiNote, NoteEvent, Velocity};

    fn note(n: u8) -> MidiNote {
        MidiNote::new(n).unwrap()
    }
    fn vel(v: u8) -> Velocity {
        Velocity::new(v).unwrap()
    }
    fn on(n: u8, ts: u64) -> NoteEvent {
        NoteEvent::on(note(n), vel(64), ts)
    }
    fn off(n: u8, ts: u64) -> NoteEvent {
        NoteEvent::off(note(n), ts)
    }

    #[test]
    fn new_buffer_is_empty() {
        let buf = EventBuffer::new();
        assert!(buf.is_empty());
        assert_eq!(buf.len(), 0);
        assert_eq!(buf.events(), &[]);
    }

    #[test]
    fn push_in_order_preserves_order() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 0));
        buf.push(on(62, 1_000));
        buf.push(off(60, 2_000));

        assert_eq!(buf.len(), 3);
        assert_eq!(buf.events()[0].timestamp_us, 0);
        assert_eq!(buf.events()[1].timestamp_us, 1_000);
        assert_eq!(buf.events()[2].timestamp_us, 2_000);
    }

    #[test]
    fn push_out_of_order_is_sorted() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 2_000));
        buf.push(on(62, 0)); // earlier — should be inserted at front
        buf.push(off(60, 1_000));

        let ts: Vec<u64> = buf.events().iter().map(|e| e.timestamp_us).collect();
        assert_eq!(ts, vec![0, 1_000, 2_000]);
    }

    #[test]
    fn events_in_range_exact() {
        let mut buf = EventBuffer::new();
        for ts in [0, 1_000, 2_000, 3_000, 4_000] {
            buf.push(on(60, ts));
        }

        let slice = buf.events_in_range(1_000, 3_000);
        assert_eq!(slice.len(), 3);
        assert_eq!(slice[0].timestamp_us, 1_000);
        assert_eq!(slice[2].timestamp_us, 3_000);
    }

    #[test]
    fn events_in_range_empty_when_no_match() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 500));
        buf.push(on(60, 1_500));

        assert_eq!(buf.events_in_range(2_000, 3_000), &[]);
    }

    #[test]
    fn clear_empties_buffer() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 0));
        buf.push(on(62, 1_000));
        buf.clear();

        assert!(buf.is_empty());
        assert_eq!(buf.events(), &[]);
    }

    #[test]
    fn drain_returns_events_and_clears() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 0));
        buf.push(off(60, 500));

        let drained = buf.drain();
        assert_eq!(drained.len(), 2);
        assert!(buf.is_empty());
    }

    #[test]
    fn default_is_empty() {
        let buf = EventBuffer::default();
        assert!(buf.is_empty());
    }

    #[test]
    fn same_timestamp_events_both_stored() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 1_000));
        buf.push(on(64, 1_000)); // chord: same timestamp
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn duration_us_empty_is_zero() {
        assert_eq!(EventBuffer::new().duration_us(), 0);
    }

    #[test]
    fn duration_us_returns_max_timestamp() {
        let mut buf = EventBuffer::new();
        buf.push(on(60, 1_000));
        buf.push(off(60, 5_000));
        buf.push(on(62, 3_000));
        assert_eq!(buf.duration_us(), 5_000);
    }
}
