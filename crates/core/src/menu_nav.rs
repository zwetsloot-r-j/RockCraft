//! Navigating menus from the piano itself (no computer keyboard needed).
//!
//! Four piano keys stand in for the four menu keys, by pitch class — so the
//! key works in any octave, on any size of keyboard:
//!
//! | Piano key | Menu key | Computer-key equivalent |
//! |-----------|----------|-------------------------|
//! | C         | Confirm  | Enter                   |
//! | C#        | Cancel   | Esc                     |
//! | D         | Up       | ↑                       |
//! | E         | Down     | ↓                       |
//!
//! The mapping is pure; each frontend decides *which* screens are menus (the
//! play and edit screens never navigate, since there every key is music) and
//! turns a [`MenuNav`] into its own equivalent keypress.

use crate::events::{NoteEvent, NoteEventKind};

/// One menu-navigation key, played on the piano.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MenuNav {
    /// Activate the highlighted item (piano C).
    Confirm,
    /// Back out / close (piano C#).
    Cancel,
    /// Move the highlight up (piano D).
    Up,
    /// Move the highlight down (piano E).
    Down,
}

impl MenuNav {
    /// Every menu key, in table order.
    pub const ALL: [MenuNav; 4] = [
        MenuNav::Confirm,
        MenuNav::Cancel,
        MenuNav::Up,
        MenuNav::Down,
    ];

    /// The menu key a MIDI note number stands for, in any octave; `None` for
    /// every other key.
    pub const fn from_note(note: u8) -> Option<Self> {
        match note % 12 {
            0 => Some(MenuNav::Confirm),
            1 => Some(MenuNav::Cancel),
            2 => Some(MenuNav::Up),
            4 => Some(MenuNav::Down),
            _ => None,
        }
    }

    /// The menu key a played event stands for. Only a key going *down*
    /// navigates: releases (and velocity-0 note-ons, MIDI's other note-off)
    /// never do, so one press is one step.
    pub fn from_event(ev: &NoteEvent) -> Option<Self> {
        match ev.kind {
            NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                Self::from_note(ev.note.value())
            }
            _ => None,
        }
    }

    /// The pitch class (0 = C) of the piano key for this menu key.
    pub const fn pitch_class(self) -> u8 {
        match self {
            MenuNav::Confirm => 0,
            MenuNav::Cancel => 1,
            MenuNav::Up => 2,
            MenuNav::Down => 4,
        }
    }

    /// Stable lower-case wire name (`"confirm"`, `"cancel"`, `"up"`, `"down"`).
    pub const fn name(self) -> &'static str {
        match self {
            MenuNav::Confirm => "confirm",
            MenuNav::Cancel => "cancel",
            MenuNav::Up => "up",
            MenuNav::Down => "down",
        }
    }

    /// The piano key's note letter, for on-screen hints (`"C"`, `"C#"`, …).
    pub const fn key_label(self) -> &'static str {
        match self {
            MenuNav::Confirm => "C",
            MenuNav::Cancel => "C#",
            MenuNav::Up => "D",
            MenuNav::Down => "E",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MidiNote, Velocity};

    fn on(note: u8, vel: u8) -> NoteEvent {
        NoteEvent {
            note: MidiNote::new(note).unwrap(),
            kind: NoteEventKind::On {
                velocity: Velocity::new(vel).unwrap(),
            },
            timestamp_us: 0,
        }
    }

    fn off(note: u8) -> NoteEvent {
        NoteEvent {
            note: MidiNote::new(note).unwrap(),
            kind: NoteEventKind::Off,
            timestamp_us: 0,
        }
    }

    #[test]
    fn middle_octave_maps_c_cs_d_e() {
        assert_eq!(MenuNav::from_note(60), Some(MenuNav::Confirm)); // C4
        assert_eq!(MenuNav::from_note(61), Some(MenuNav::Cancel)); // C#4
        assert_eq!(MenuNav::from_note(62), Some(MenuNav::Up)); // D4
        assert_eq!(MenuNav::from_note(64), Some(MenuNav::Down)); // E4
    }

    #[test]
    fn other_keys_do_nothing() {
        for n in [63, 65, 66, 67, 68, 69, 70, 71] {
            assert_eq!(MenuNav::from_note(n), None, "note {n}");
        }
    }

    #[test]
    fn works_in_every_octave() {
        for octave in 0..=9u8 {
            let base = octave * 12;
            assert_eq!(MenuNav::from_note(base), Some(MenuNav::Confirm));
            assert_eq!(MenuNav::from_note(base + 1), Some(MenuNav::Cancel));
            assert_eq!(MenuNav::from_note(base + 2), Some(MenuNav::Up));
            assert_eq!(MenuNav::from_note(base + 4), Some(MenuNav::Down));
        }
        // Lowest C on an 88-key piano (C1) and the top note (C8).
        assert_eq!(MenuNav::from_note(24), Some(MenuNav::Confirm));
        assert_eq!(MenuNav::from_note(108), Some(MenuNav::Confirm));
    }

    #[test]
    fn only_key_down_navigates() {
        assert_eq!(MenuNav::from_event(&on(60, 90)), Some(MenuNav::Confirm));
        assert_eq!(MenuNav::from_event(&off(60)), None);
        // Velocity-0 note-on is a note-off by MIDI convention.
        assert_eq!(MenuNav::from_event(&on(60, 0)), None);
        assert_eq!(MenuNav::from_event(&on(65, 90)), None);
    }

    #[test]
    fn pitch_class_round_trips() {
        for nav in MenuNav::ALL {
            assert_eq!(MenuNav::from_note(nav.pitch_class()), Some(nav));
        }
    }

    #[test]
    fn names_are_stable() {
        let names: Vec<_> = MenuNav::ALL.iter().map(|n| n.name()).collect();
        assert_eq!(names, ["confirm", "cancel", "up", "down"]);
        let labels: Vec<_> = MenuNav::ALL.iter().map(|n| n.key_label()).collect();
        assert_eq!(labels, ["C", "C#", "D", "E"]);
    }
}
