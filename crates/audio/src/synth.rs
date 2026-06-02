//! SoundFont piano synth — turns `NoteEvent`s into sound.
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

use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::time::Duration;

use rockcraft_core::{MidiNote, NoteEvent, NoteEventKind, Velocity};
use rodio::Source;
use rustysynth::{SoundFont, Synthesizer, SynthesizerSettings};

/// We render everything on one MIDI channel — a single piano.
const MIDI_CHANNEL: i32 = 0;

/// A command handed from the app thread to the audio thread. Tiny and `Copy`
/// so enqueuing is cheap and allocation-free.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SynthCommand {
    NoteOn { note: u8, velocity: u8 },
    NoteOff { note: u8 },
    AllOff,
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
#[derive(Clone)]
pub struct SynthHandle {
    tx: Sender<SynthCommand>,
}

impl SynthHandle {
    /// Start sounding `note` at `velocity`.
    pub fn note_on(&self, note: MidiNote, velocity: Velocity) {
        let _ = self.tx.send(SynthCommand::NoteOn {
            note: note.value(),
            velocity: velocity.value(),
        });
    }

    /// Release `note`.
    pub fn note_off(&self, note: MidiNote) {
        let _ = self.tx.send(SynthCommand::NoteOff { note: note.value() });
    }

    /// Release everything (panic button / screen change).
    pub fn all_off(&self) {
        let _ = self.tx.send(SynthCommand::AllOff);
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
                Ok(SynthCommand::NoteOn { note, velocity }) => {
                    self.synth
                        .note_on(MIDI_CHANNEL, note as i32, velocity as i32);
                }
                Ok(SynthCommand::NoteOff { note }) => {
                    self.synth.note_off(MIDI_CHANNEL, note as i32);
                }
                Ok(SynthCommand::AllOff) => self.synth.note_off_all(false),
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
pub fn synth_from_sf2_bytes(
    bytes: &[u8],
    sample_rate: u32,
) -> Result<(SynthSource, SynthHandle), SynthError> {
    let mut reader = bytes;
    let sound_font =
        Arc::new(SoundFont::new(&mut reader).map_err(|e| SynthError::SoundFont(e.to_string()))?);
    let settings = SynthesizerSettings::new(sample_rate as i32);
    let synth =
        Synthesizer::new(&sound_font, &settings).map_err(|e| SynthError::Synth(e.to_string()))?;
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
    Ok((source, SynthHandle { tx }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drain(rx: &Receiver<SynthCommand>) -> Vec<SynthCommand> {
        rx.try_iter().collect()
    }

    #[test]
    fn handle_enqueues_in_order() {
        let (tx, rx) = mpsc::channel();
        let h = SynthHandle { tx };
        let c4 = MidiNote::new(60).unwrap();
        h.note_on(c4, Velocity::new(100).unwrap());
        h.note_off(c4);
        assert_eq!(
            drain(&rx),
            vec![
                SynthCommand::NoteOn {
                    note: 60,
                    velocity: 100
                },
                SynthCommand::NoteOff { note: 60 },
            ]
        );
    }

    #[test]
    fn apply_maps_note_events() {
        let (tx, rx) = mpsc::channel();
        let h = SynthHandle { tx };
        let c4 = MidiNote::new(60).unwrap();
        h.apply(&NoteEvent::on(c4, Velocity::new(80).unwrap(), 0));
        h.apply(&NoteEvent::off(c4, 10));
        assert_eq!(
            drain(&rx),
            vec![
                SynthCommand::NoteOn {
                    note: 60,
                    velocity: 80
                },
                SynthCommand::NoteOff { note: 60 },
            ]
        );
    }

    #[test]
    fn apply_velocity_zero_on_is_note_off() {
        let (tx, rx) = mpsc::channel();
        let h = SynthHandle { tx };
        let c4 = MidiNote::new(60).unwrap();
        // A note-on with velocity 0 is, by MIDI convention, a note-off.
        h.apply(&NoteEvent::on(c4, Velocity::new(0).unwrap(), 0));
        assert_eq!(drain(&rx), vec![SynthCommand::NoteOff { note: 60 }]);
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
