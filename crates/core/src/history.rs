//! Bounded undo/redo history over [`Timeline`] snapshots.
//!
//! ## Checkpoint discipline
//!
//! Call [`History::checkpoint`] *before* each mutation. This saves the
//! pre-mutation state onto the undo stack and clears the redo stack, giving a
//! linear history: every edit is undoable, and redo is only available
//! immediately after an undo.
//!
//! ## Transient (preview) mutations
//!
//! For multi-step previews that may be cancelled (e.g. the chord selector):
//!
//! 1. Call `checkpoint` once before entering preview mode.
//! 2. Mutate `current_mut()` freely during the preview.
//! 3. **Commit**: do nothing extra — the clean checkpoint is already on the
//!    undo stack; the committed notes are already in `current`.
//! 4. **Cancel**: call `rollback()` — discards all preview mutations and
//!    restores the clean state without touching the redo stack.

use crate::timeline::Timeline;

/// Bounded undo/redo history over [`Timeline`] snapshots.
pub struct History {
    /// Undo stack: oldest at index 0, most recent at the back.
    past: Vec<Timeline>,
    /// The live, currently-editing state.
    current: Timeline,
    /// Redo stack: next-to-apply at the back, furthest at index 0.
    future: Vec<Timeline>,
    /// Maximum entries on the undo stack; oldest are dropped when exceeded.
    capacity: usize,
}

impl History {
    /// Wrap `timeline` in a fresh history with a bounded undo stack of
    /// `capacity` entries.
    pub fn new(timeline: Timeline, capacity: usize) -> Self {
        Self {
            past: Vec::new(),
            current: timeline,
            future: Vec::new(),
            capacity,
        }
    }

    /// Borrow the current (live-editing) timeline.
    pub fn current(&self) -> &Timeline {
        &self.current
    }

    /// Mutably borrow the current timeline for in-place edits.
    ///
    /// Does **not** automatically checkpoint — call
    /// [`checkpoint`](Self::checkpoint) before the mutation if the old state
    /// should be undoable.
    pub fn current_mut(&mut self) -> &mut Timeline {
        &mut self.current
    }

    /// Save the current state as a checkpoint *before* a mutation, clearing
    /// the redo stack. If the undo stack would exceed `capacity`, the oldest
    /// entry is silently dropped.
    pub fn checkpoint(&mut self) {
        if self.past.len() >= self.capacity {
            self.past.remove(0);
        }
        self.past.push(self.current.clone());
        self.future.clear();
    }

    /// Restore the most recent checkpoint, pushing the current state onto the
    /// redo stack. Returns `false` (no-op) if there is nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(prev) = self.past.pop() else {
            return false;
        };
        let was = std::mem::replace(&mut self.current, prev);
        self.future.push(was);
        true
    }

    /// Restore the most recently undone state, pushing the current state back
    /// onto the undo stack. Returns `false` (no-op) if there is nothing to
    /// redo.
    pub fn redo(&mut self) -> bool {
        let Some(next) = self.future.pop() else {
            return false;
        };
        let was = std::mem::replace(&mut self.current, next);
        self.past.push(was);
        true
    }

    /// Discard the current state and restore the most recent checkpoint
    /// *without* placing the discarded state on the redo stack. Used to cancel
    /// transient preview mutations so the redo stack is unaffected. Returns
    /// `false` if there is no checkpoint to roll back to.
    pub fn rollback(&mut self) -> bool {
        let Some(prev) = self.past.pop() else {
            return false;
        };
        self.current = prev;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MidiNote, Velocity};
    use crate::timeline::{Note, Timeline};

    fn note(pitch: u8) -> Note {
        Note {
            pitch: MidiNote::new(pitch).unwrap(),
            start_us: 0,
            dur_us: 1_000,
            velocity: Velocity::new(80).unwrap(),
        }
    }

    /// After N checkpointed edits, undo restores each previous state in
    /// reverse order; redo then replays them forward.
    #[test]
    fn undo_redo_cycle() {
        let mut h = History::new(Timeline::new(), 10);

        h.checkpoint();
        h.current_mut().insert(note(60));
        h.checkpoint();
        h.current_mut().insert(note(62));
        h.checkpoint();
        h.current_mut().insert(note(64));

        assert_eq!(h.current().len(), 3);

        assert!(h.undo());
        assert_eq!(h.current().len(), 2);
        assert!(h.undo());
        assert_eq!(h.current().len(), 1);
        assert!(h.undo());
        assert_eq!(h.current().len(), 0);
        assert!(!h.undo(), "nothing left to undo");

        assert!(h.redo());
        assert_eq!(h.current().len(), 1);
        assert!(h.redo());
        assert_eq!(h.current().len(), 2);
        assert!(h.redo());
        assert_eq!(h.current().len(), 3);
        assert!(!h.redo(), "nothing left to redo");
    }

    /// A new edit after undo clears the redo stack.
    #[test]
    fn new_edit_after_undo_clears_redo() {
        let mut h = History::new(Timeline::new(), 10);

        h.checkpoint();
        h.current_mut().insert(note(60));
        h.checkpoint();
        h.current_mut().insert(note(62));

        assert!(h.undo());
        assert_eq!(h.current().len(), 1);

        h.checkpoint();
        h.current_mut().insert(note(64));
        assert_eq!(h.current().len(), 2);

        assert!(!h.redo(), "redo cleared by the new edit");
    }

    /// The undo stack is bounded to `capacity`; oldest entries are silently
    /// dropped. `undo` and `redo` never panic on an empty stack.
    #[test]
    fn bounded_capacity_and_empty_return_false() {
        let mut h = History::new(Timeline::new(), 10);
        assert!(!h.undo(), "empty undo returns false");
        assert!(!h.redo(), "empty redo returns false");

        let capacity = 3usize;
        let mut h2 = History::new(Timeline::new(), capacity);
        for i in 0..5u8 {
            h2.checkpoint();
            h2.current_mut().insert(note(60 + i));
        }
        assert_eq!(h2.current().len(), 5);

        for _ in 0..capacity {
            assert!(h2.undo());
        }
        assert!(
            !h2.undo(),
            "oldest entries were dropped — bounded to capacity"
        );
    }

    /// `rollback` restores the last checkpoint without pushing to the redo
    /// stack, so transient preview mutations vanish cleanly.
    #[test]
    fn rollback_discards_transient_mutations() {
        let mut h = History::new(Timeline::new(), 10);

        // First permanent edit.
        h.checkpoint();
        h.current_mut().insert(note(60));

        // Entering preview mode: checkpoint the 1-note state, then preview.
        h.checkpoint();
        h.current_mut().insert(note(62));
        assert_eq!(h.current().len(), 2);

        // Cancel: rollback to the 1-note state.
        assert!(h.rollback());
        assert_eq!(h.current().len(), 1, "preview note discarded");

        // Rollback must not push to the redo stack.
        assert!(!h.redo(), "rollback does not push to the redo stack");

        // Undo still works for the permanent edit.
        assert!(h.undo());
        assert_eq!(h.current().len(), 0, "underlying edit still undoable");
    }

    /// A single checkpoint covering multiple note insertions undoes them all
    /// as one step (the chord-insert contract).
    #[test]
    fn multi_note_checkpoint_undoes_as_one_step() {
        let mut h = History::new(Timeline::new(), 10);

        h.checkpoint();
        h.current_mut().insert(note(60));
        h.current_mut().insert(note(64));
        h.current_mut().insert(note(67));

        assert_eq!(h.current().len(), 3);
        assert!(h.undo());
        assert_eq!(h.current().len(), 0, "all three notes removed in one undo");
        assert!(h.redo());
        assert_eq!(h.current().len(), 3, "all three notes restored in one redo");
    }
}
