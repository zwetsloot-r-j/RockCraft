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
//! | A0        | Free play on / off | —             |
//!
//! A0 — the very lowest key of an 88-key piano ([`FREE_PLAY_NOTE`]) — toggles
//! *free play*: while it is on, C / C# / D / E are just notes, so you can play
//! freely on a menu without moving through it. Press A0 again to navigate.
//! [`MenuNavigator`] holds that state.
//!
//! The mapping is pure; each frontend decides *which* screens are menus (the
//! play and edit screens never navigate, since there every key is music) and
//! turns a [`MenuNav`] into its own equivalent keypress.

use crate::events::{NoteEvent, NoteEventKind};

/// The key that toggles free play: A0, the lowest key of an 88-key piano.
pub const FREE_PLAY_NOTE: u8 = 21;

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
    /// Turn free play on / off (piano A0, [`FREE_PLAY_NOTE`]).
    ToggleFreePlay,
}

impl MenuNav {
    /// Every menu key, in table order.
    pub const ALL: [MenuNav; 5] = [
        MenuNav::Confirm,
        MenuNav::Cancel,
        MenuNav::Up,
        MenuNav::Down,
        MenuNav::ToggleFreePlay,
    ];

    /// The menu key a MIDI note number stands for; `None` for every other key.
    /// A0 toggles free play; otherwise C / C# / D / E in any octave navigate.
    pub const fn from_note(note: u8) -> Option<Self> {
        if note == FREE_PLAY_NOTE {
            return Some(MenuNav::ToggleFreePlay);
        }
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

    /// Stable lower-case wire name (`"confirm"`, `"cancel"`, `"up"`, `"down"`,
    /// `"free_play"`).
    pub const fn name(self) -> &'static str {
        match self {
            MenuNav::Confirm => "confirm",
            MenuNav::Cancel => "cancel",
            MenuNav::Up => "up",
            MenuNav::Down => "down",
            MenuNav::ToggleFreePlay => "free_play",
        }
    }

    /// The piano key's name, for on-screen hints (`"C"`, `"C#"`, …, `"A0"`).
    pub const fn key_label(self) -> &'static str {
        match self {
            MenuNav::Confirm => "C",
            MenuNav::Cancel => "C#",
            MenuNav::Up => "D",
            MenuNav::Down => "E",
            MenuNav::ToggleFreePlay => "A0",
        }
    }
}

/// Piano menu navigation with its free-play switch.
///
/// Feed it each [`MenuNav`] played on a menu screen; it answers with the
/// navigation to perform, or `None` when free play swallows it (or the press
/// was the toggle itself).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MenuNavigator {
    free_play: bool,
}

impl MenuNavigator {
    /// Navigation on, free play off.
    pub const fn new() -> Self {
        Self { free_play: false }
    }

    /// Whether free play is on (piano keys don't navigate).
    pub const fn is_free_play(&self) -> bool {
        self.free_play
    }

    /// Turn free play on or off directly.
    pub fn set_free_play(&mut self, on: bool) {
        self.free_play = on;
    }

    /// Apply one press: A0 flips free play and navigates nothing; C / C# / D /
    /// E come back to act on unless free play is on.
    pub fn press(&mut self, nav: MenuNav) -> Option<MenuNav> {
        match nav {
            MenuNav::ToggleFreePlay => {
                self.free_play = !self.free_play;
                None
            }
            _ if self.free_play => None,
            other => Some(other),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::{MidiNote, Velocity};

    fn on(note: u8, vel: u8) -> NoteEvent {
        NoteEvent::on(MidiNote::new(note).unwrap(), Velocity::new(vel).unwrap(), 0)
    }

    fn off(note: u8) -> NoteEvent {
        NoteEvent::off(MidiNote::new(note).unwrap(), 0)
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
        // Only the lowest A toggles — every other A is just a note.
        for n in [33, 45, 57, 69, 81, 93, 105] {
            assert_eq!(MenuNav::from_note(n), None, "A at {n}");
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
    fn lowest_piano_key_toggles_free_play() {
        assert_eq!(FREE_PLAY_NOTE, 21);
        assert_eq!(MidiNote::new(FREE_PLAY_NOTE).unwrap().name(), "A0");
        assert_eq!(MenuNav::from_note(21), Some(MenuNav::ToggleFreePlay));
        assert_eq!(
            MenuNav::from_event(&on(21, 90)),
            Some(MenuNav::ToggleFreePlay)
        );
        assert_eq!(MenuNav::from_event(&off(21)), None);
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
    fn names_are_stable() {
        let names: Vec<_> = MenuNav::ALL.iter().map(|n| n.name()).collect();
        assert_eq!(names, ["confirm", "cancel", "up", "down", "free_play"]);
        let labels: Vec<_> = MenuNav::ALL.iter().map(|n| n.key_label()).collect();
        assert_eq!(labels, ["C", "C#", "D", "E", "A0"]);
    }

    #[test]
    fn navigator_passes_keys_through_until_free_play() {
        let mut nav = MenuNavigator::new();
        assert!(!nav.is_free_play());
        assert_eq!(nav.press(MenuNav::Down), Some(MenuNav::Down));

        assert_eq!(nav.press(MenuNav::ToggleFreePlay), None);
        assert!(nav.is_free_play());
        for key in [
            MenuNav::Confirm,
            MenuNav::Cancel,
            MenuNav::Up,
            MenuNav::Down,
        ] {
            assert_eq!(nav.press(key), None, "{key:?} swallowed in free play");
        }

        assert_eq!(nav.press(MenuNav::ToggleFreePlay), None);
        assert!(!nav.is_free_play());
        assert_eq!(nav.press(MenuNav::Confirm), Some(MenuNav::Confirm));
    }
}
