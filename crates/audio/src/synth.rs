//! SoundFont synth — turns `NoteEvent`s into sound, on two independent buses.
//!
//! Two halves, split across the real-time boundary (see `CLAUDE.md`):
//! - [`SynthHandle`] lives on the MIDI/app thread. Its methods only **enqueue**
//!   a tiny [`SynthCommand`] onto a channel — no rendering, no locks, no I/O —
//!   so calling it from the event-routing loop can never stall.
//! - [`SynthSource`] lives on rodio's audio thread. As an infinite
//!   [`rodio::Source`], its `next()` drains pending commands into the
//!   `rustysynth` [`Synthesizer`] and renders the next block of interleaved
//!   stereo samples.
//!
//! Construct both with [`synth_from_sf2_bytes`]; the pieces are wired together
//! by a single SPSC channel.
//!
//! **Buses (M14-C).** Every handle carries a [`SynthBus`] — the notes you play
//! (`Player`) or the notes the song plays (`Song`) — and addresses that bus's
//! MIDI channel. Each channel has its own program (instrument) and channel
//! volume, so the two can sound different and sit at different levels. Get the
//! other bus's handle with [`SynthHandle::for_bus`]; the handle returned by
//! [`synth_from_sf2_bytes`] starts on [`SynthBus::Player`].

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use rockcraft_core::{Gain, Instrument, MidiNote, NoteEvent, NoteEventKind, SynthBus, Velocity};
use rodio::Source;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

/// MIDI status byte for a program change (instrument select) on a channel.
const PROGRAM_CHANGE: i32 = 0xC0;
/// MIDI status byte for a control change.
const CONTROL_CHANGE: i32 = 0xB0;
/// Controller 7: channel volume — how a bus's level is set on the synth.
const CC_CHANNEL_VOLUME: i32 = 7;

/// A command handed from the app thread to the audio thread. Tiny and `Copy`
/// so enqueuing is cheap and allocation-free. `channel` is the bus's MIDI
/// channel ([`SynthBus::midi_channel`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SynthCommand {
    NoteOn {
        channel: u8,
        note: u8,
        velocity: u8,
    },
    NoteOff {
        channel: u8,
        note: u8,
    },
    /// Release every note on every bus (panic button / screen change).
    AllOff,
    /// Select a General MIDI program on one bus.
    Program {
        channel: u8,
        program: u8,
    },
    /// Set one bus's channel volume (controller 7), `0..=127`.
    Volume {
        channel: u8,
        value: u8,
    },
}

/// Errors building a synth from SoundFont bytes.
#[derive(Debug)]
pub enum SynthError {
    /// The bytes were not a valid SoundFont.
    SoundFont(String),
    /// `rustysynth` rejected the synthesizer settings / SoundFont.
    Synth(String),
}

impl std::fmt::Display for SynthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SynthError::SoundFont(e) => write!(f, "invalid SoundFont: {e}"),
            SynthError::Synth(e) => write!(f, "synth init failed: {e}"),
        }
    }
}

impl std::error::Error for SynthError {}

/// Cheap, cloneable handle used from the MIDI/app thread. Every method just
/// enqueues a command; if the audio thread is gone the send is silently
/// dropped (we're shutting down).
///
/// A handle is *bound to one bus*: its notes, instrument, and level all address
/// that bus's MIDI channel. [`for_bus`](SynthHandle::for_bus) hands back a
/// sibling on the other bus over the same channel to the audio thread.
#[derive(Clone)]
pub struct SynthHandle {
    tx: Sender<SynthCommand>,
    bus: SynthBus,
}

impl SynthHandle {
    /// The bus this handle plays on.
    pub fn bus(&self) -> SynthBus {
        self.bus
    }

    /// A handle onto another bus of the same synth — e.g. the song voice, from
    /// the player voice. Cheap: it clones the command sender, nothing more.
    pub fn for_bus(&self, bus: SynthBus) -> SynthHandle {
        SynthHandle {
            tx: self.tx.clone(),
            bus,
        }
    }

    /// Start sounding `note` at `velocity` on this handle's bus.
    pub fn note_on(&self, note: MidiNote, velocity: Velocity) {
        let _ = self.tx.send(SynthCommand::NoteOn {
            channel: self.bus.midi_channel(),
            note: note.value(),
            velocity: velocity.value(),
        });
    }

    /// Release `note` on this handle's bus.
    pub fn note_off(&self, note: MidiNote) {
        let _ = self.tx.send(SynthCommand::NoteOff {
            channel: self.bus.midi_channel(),
            note: note.value(),
        });
    }

    /// Release everything, on **every** bus (panic button / screen change).
    pub fn all_off(&self) {
        let _ = self.tx.send(SynthCommand::AllOff);
    }

    /// Switch this bus to `instrument` (a General MIDI program change).
    ///
    /// Whether it is audible depends on the loaded SoundFont: a full GM bank
    /// has all the programs, while a single-preset piano bank falls back to its
    /// one preset (see `crates/audio/assets/NOTICE.md`).
    pub fn set_instrument(&self, instrument: &Instrument) {
        let _ = self.tx.send(SynthCommand::Program {
            channel: self.bus.midi_channel(),
            program: instrument.program,
        });
    }

    /// Set this bus's level (MIDI channel volume). Notes already sounding
    /// follow the new level; the other bus is untouched.
    pub fn set_gain(&self, gain: Gain) {
        let _ = self.tx.send(SynthCommand::Volume {
            channel: self.bus.midi_channel(),
            value: gain.midi_volume(),
        });
    }

    /// Route a [`NoteEvent`] straight to the synth. A note-on with velocity 0 is
    /// treated as a note-off, mirroring the MIDI convention used everywhere else.
    pub fn apply(&self, ev: &NoteEvent) {
        match ev.kind {
            NoteEventKind::On { velocity } if !velocity.is_note_off() => {
                self.note_on(ev.note, velocity)
            }
            _ => self.note_off(ev.note),
        }
    }
}

/// The audio-thread half: owns the synthesizer and renders on demand.
///
/// An infinite source — it always produces samples (silence when nothing is
/// held), so rodio keeps it alive for the life of the stream.
pub struct SynthSource {
    synth: Synthesizer,
    rx: Receiver<SynthCommand>,
    sample_rate: u32,
    // Scratch buffers for one rendered block: rustysynth fills separate L/R,
    // we interleave into `frame` and hand samples out one at a time.
    left: Vec<f32>,
    right: Vec<f32>,
    frame: Vec<f32>,
    // Read cursor into `frame`; when it reaches the end we render the next block.
    pos: usize,
}

impl SynthSource {
    /// Drain queued commands and render the next block into `frame`.
    fn refill(&mut self) {
        loop {
            match self.rx.try_recv() {
                Ok(SynthCommand::NoteOn {
                    channel,
                    note,
                    velocity,
                }) => {
                    self.synth
                        .note_on(channel as i32, note as i32, velocity as i32);
                }
                Ok(SynthCommand::NoteOff { channel, note }) => {
                    self.synth.note_off(channel as i32, note as i32);
                }
                Ok(SynthCommand::AllOff) => self.synth.note_off_all(false),
                Ok(SynthCommand::Program { channel, program }) => {
                    self.synth.process_midi_message(
                        channel as i32,
                        PROGRAM_CHANGE,
                        program as i32,
                        0,
                    );
                }
                Ok(SynthCommand::Volume { channel, value }) => {
                    self.synth.process_midi_message(
                        channel as i32,
                        CONTROL_CHANGE,
                        CC_CHANNEL_VOLUME,
                        value as i32,
                    );
                }
                // Nothing pending, or the handle was dropped: stop draining and
                // render whatever is currently sounding (release tails, silence).
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        self.synth.render(&mut self.left, &mut self.right);
        interleave(&self.left, &self.right, &mut self.frame);
        self.pos = 0;
    }
}

impl Iterator for SynthSource {
    type Item = f32;

    fn next(&mut self) -> Option<f32> {
        if self.pos >= self.frame.len() {
            self.refill();
        }
        let sample = self.frame[self.pos];
        self.pos += 1;
        Some(sample)
    }
}

impl Source for SynthSource {
    fn current_frame_len(&self) -> Option<usize> {
        // Unknown / continuous — we never stop producing samples.
        None
    }

    fn channels(&self) -> u16 {
        2
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        // Infinite.
        None
    }
}

/// Interleave separate left/right blocks into `out` (`L,R,L,R,…`). `out` is
/// reused across calls to avoid per-block allocation.
fn interleave(left: &[f32], right: &[f32], out: &mut Vec<f32>) {
    out.clear();
    for (l, r) in left.iter().zip(right.iter()) {
        out.push(*l);
        out.push(*r);
    }
}

/// Build a synth from SoundFont bytes, returning the audio-thread [`SynthSource`]
/// (hand to `rodio`) and the app-thread [`SynthHandle`] (call from the event
/// loop). `sample_rate` is the output rate the source renders at.
///
/// The handle comes back on [`SynthBus::Player`]; reach the song voice with
/// [`SynthHandle::for_bus`].
pub fn synth_from_sf2_bytes(
    bytes: &[u8],
    sample_rate: u32,
) -> Result<(SynthSource, SynthHandle), SynthError> {
    let mut reader = bytes;
    let sound_font =
        Arc::new(SoundFont::new(&mut reader).map_err(|e| SynthError::SoundFont(e.to_string()))?);
    let settings = SynthesizerSettings::new(sample_rate as i32);
    let mut synth =
        Synthesizer::new(&sound_font, &settings).map_err(|e| SynthError::Synth(e.to_string()))?;
    // Optional distinct instrument: set `ROCKCRAFT_SYNTH_PROGRAM` to a General
    // MIDI program number (e.g. 12 = marimba) to make the synth's notes stand
    // out over a same-timbre backing — useful for checking a chart's alignment
    // against the source audio by ear. Unset ⇒ the SoundFont's default (piano).
    // It seeds *both* buses; the mixer's per-bus instrument overrides it live.
    if let Some(prog) = std::env::var("ROCKCRAFT_SYNTH_PROGRAM")
        .ok()
        .and_then(|s| s.trim().parse::<i32>().ok())
    {
        for bus in SynthBus::all() {
            synth.process_midi_message(bus.midi_channel() as i32, PROGRAM_CHANGE, prog, 0);
        }
    }
    let block = synth.get_block_size();

    let (tx, rx) = mpsc::channel();
    let source = SynthSource {
        synth,
        rx,
        sample_rate,
        left: vec![0.0; block],
        right: vec![0.0; block],
        frame: Vec::with_capacity(block * 2),
        pos: 0, // empty `frame` forces a render on the first `next()`
    };
    Ok((
        source,
        SynthHandle {
            tx,
            bus: SynthBus::Player,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(rx: &Receiver<SynthCommand>) -> Vec<SynthCommand> {
        rx.try_iter().collect()
    }

    /// A player-bus handle plus the receiving end the audio thread would own.
    fn handle() -> (SynthHandle, Receiver<SynthCommand>) {
        let (tx, rx) = mpsc::channel();
        (
            SynthHandle {
                tx,
                bus: SynthBus::Player,
            },
            rx,
        )
    }

    const PLAYER: u8 = 0;
    const SONG: u8 = 1;

    #[test]
    fn handle_enqueues_in_order() {
        let (h, rx) = handle();
        let c4 = MidiNote::new(60).unwrap();
        h.note_on(c4, Velocity::new(100).unwrap());
        h.note_off(c4);
        assert_eq!(
            drain(&rx),
            vec![
                SynthCommand::NoteOn {
                    channel: PLAYER,
                    note: 60,
                    velocity: 100
                },
                SynthCommand::NoteOff {
                    channel: PLAYER,
                    note: 60
                },
            ]
        );
    }

    #[test]
    fn apply_maps_note_events() {
        let (h, rx) = handle();
        let c4 = MidiNote::new(60).unwrap();
        h.apply(&NoteEvent::on(c4, Velocity::new(80).unwrap(), 0));
        h.apply(&NoteEvent::off(c4, 10));
        assert_eq!(
            drain(&rx),
            vec![
                SynthCommand::NoteOn {
                    channel: PLAYER,
                    note: 60,
                    velocity: 80
                },
                SynthCommand::NoteOff {
                    channel: PLAYER,
                    note: 60
                },
            ]
        );
    }

    #[test]
    fn apply_velocity_zero_on_is_note_off() {
        let (h, rx) = handle();
        let c4 = MidiNote::new(60).unwrap();
        // A note-on with velocity 0 is, by MIDI convention, a note-off.
        h.apply(&NoteEvent::on(c4, Velocity::new(0).unwrap(), 0));
        assert_eq!(
            drain(&rx),
            vec![SynthCommand::NoteOff {
                channel: PLAYER,
                note: 60
            }]
        );
    }

    #[test]
    fn a_handle_starts_on_the_player_bus() {
        let (h, _rx) = handle();
        assert_eq!(h.bus(), SynthBus::Player);
    }

    #[test]
    fn for_bus_addresses_the_other_channel_over_the_same_queue() {
        let (player, rx) = handle();
        let song = player.for_bus(SynthBus::Song);
        assert_eq!(song.bus(), SynthBus::Song);
        let c4 = MidiNote::new(60).unwrap();
        player.note_on(c4, Velocity::new(80).unwrap());
        song.note_on(c4, Velocity::new(80).unwrap());
        // Both land on the one audio-thread queue, tagged by bus.
        assert_eq!(
            drain(&rx),
            vec![
                SynthCommand::NoteOn {
                    channel: PLAYER,
                    note: 60,
                    velocity: 80
                },
                SynthCommand::NoteOn {
                    channel: SONG,
                    note: 60,
                    velocity: 80
                },
            ]
        );
    }

    #[test]
    fn instrument_and_gain_address_the_handles_own_bus() {
        let (player, rx) = handle();
        let song = player.for_bus(SynthBus::Song);
        song.set_instrument(rockcraft_core::instrument("marimba").unwrap());
        song.set_gain(Gain::new(0.5).unwrap());
        player.set_gain(Gain::SILENT);
        assert_eq!(
            drain(&rx),
            vec![
                SynthCommand::Program {
                    channel: SONG,
                    program: 12
                },
                SynthCommand::Volume {
                    channel: SONG,
                    value: 64
                },
                SynthCommand::Volume {
                    channel: PLAYER,
                    value: 0
                },
            ]
        );
    }

    #[test]
    fn all_off_is_bus_wide() {
        // The panic button silences everything, whichever handle sends it —
        // there is one `AllOff`, not one per channel.
        let (player, rx) = handle();
        player.for_bus(SynthBus::Song).all_off();
        assert_eq!(drain(&rx), vec![SynthCommand::AllOff]);
    }

    #[test]
    fn interleave_zips_channels() {
        let mut out = Vec::new();
        interleave(&[1.0, 2.0], &[3.0, 4.0], &mut out);
        assert_eq!(out, vec![1.0, 3.0, 2.0, 4.0]);
    }

    #[test]
    fn interleave_clears_previous_contents() {
        let mut out = vec![9.0, 9.0, 9.0];
        interleave(&[1.0], &[2.0], &mut out);
        assert_eq!(out, vec![1.0, 2.0]);
    }
}
