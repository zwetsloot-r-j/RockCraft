//! Audio for RockCraft: playback, metronome, and MIDI synthesis.
//!
//! [`AudioOut`] opens the default output device and plays a SoundFont piano
//! synth; hand its [`SynthHandle`] the `NoteEvent`s coming off the piano and you
//! hear what you play. The synth machinery lives in [`synth`].
//!
//! [`play_file`] plays a decoded audio file (wav/mp3/ogg/flac) on a separate
//! output stream, returning a [`BackingHandle`] to stop, pause, resume, or seek
//! it; [`play_file_at`] starts it seeked to a position, for syncing a backing
//! track to the highway.
//!
//! Levels are per source (M14-C): the two synth buses take theirs through
//! [`SynthHandle::set_gain`], the backing track through
//! [`BackingHandle::set_gain`]. The settings themselves are `core::Mixer`.

pub mod synth;

pub use rockcraft_core as core;
pub use synth::{synth_from_sf2_bytes, SynthError, SynthHandle, SynthSource};

use std::path::PathBuf;

use rockcraft_core::Gain;
use rodio::{OutputStream, OutputStreamHandle};

/// Output sample rate the synth renders at. rodio resamples to the device rate
/// if it differs.
const SAMPLE_RATE: u32 = 44_100;

/// Environment variable overriding the SoundFont path.
const SF2_ENV: &str = "ROCKCRAFT_SF2";
/// Default SoundFont location, relative to the workspace root (the usual
/// working directory when running `cargo run -p rockcraft-tui`).
const DEFAULT_SF2_PATH: &str = "crates/audio/assets/piano.sf2";

/// Errors starting audio output.
#[derive(Debug)]
pub enum AudioError {
    /// The SoundFont file was not found. Drop a piano `.sf2` at the path (or set
    /// `ROCKCRAFT_SF2`); see `crates/audio/assets/NOTICE.md`.
    SoundFontMissing(PathBuf),
    /// Reading the SoundFont file failed.
    Io(std::io::Error),
    /// No usable output device / stream.
    Device(String),
    /// The synth could not be built from the SoundFont bytes.
    Synth(SynthError),
    /// rodio refused to start playing the source.
    Play(String),
    /// The audio file could not be decoded (unsupported format or corrupt data).
    Decode(String),
}

impl std::fmt::Display for AudioError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioError::SoundFontMissing(p) => write!(
                f,
                "SoundFont not found at {} (set {SF2_ENV} to override)",
                p.display()
            ),
            AudioError::Io(e) => write!(f, "reading SoundFont failed: {e}"),
            AudioError::Device(e) => write!(f, "no audio output device: {e}"),
            AudioError::Synth(e) => write!(f, "{e}"),
            AudioError::Play(e) => write!(f, "could not start playback: {e}"),
            AudioError::Decode(e) => write!(f, "audio decode failed: {e}"),
        }
    }
}

impl std::error::Error for AudioError {}

/// A running audio output: holds the device stream open and the synth feeding
/// it. Drop it to stop audio. Clone [`AudioOut::synth`] to drive notes.
pub struct AudioOut {
    // Keeps the device stream (and thus the synth source) alive; dropping it
    // stops all audio.
    _stream: OutputStream,
    // Kept so the backing track can share this one device stream (a second
    // `Sink`) rather than opening its own — see `play_backing_at`.
    stream_handle: OutputStreamHandle,
    handle: SynthHandle,
}

impl AudioOut {
    /// Open the default output device and start the piano synth, loading the
    /// SoundFont from `$ROCKCRAFT_SF2` or [`DEFAULT_SF2_PATH`].
    pub fn new() -> Result<Self, AudioError> {
        let path = sf2_path();
        let bytes = std::fs::read(&path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                AudioError::SoundFontMissing(path.clone())
            } else {
                AudioError::Io(e)
            }
        })?;
        Self::from_sf2_bytes(&bytes)
    }

    /// Open the default output device and start the synth from in-memory
    /// SoundFont bytes (asset-source-agnostic; handy for tests / embedding).
    pub fn from_sf2_bytes(bytes: &[u8]) -> Result<Self, AudioError> {
        let (stream, stream_handle) =
            OutputStream::try_default().map_err(|e| AudioError::Device(e.to_string()))?;
        let (source, handle) =
            synth_from_sf2_bytes(bytes, SAMPLE_RATE).map_err(AudioError::Synth)?;
        stream_handle
            .play_raw(source)
            .map_err(|e| AudioError::Play(e.to_string()))?;
        Ok(Self {
            _stream: stream,
            stream_handle,
            handle,
        })
    }

    /// A cloneable handle for sounding notes from the app thread.
    pub fn synth(&self) -> SynthHandle {
        self.handle.clone()
    }

    /// Play a backing-track file on the **same** output stream as the synth.
    ///
    /// Sharing the synth's `OutputStream` (one device stream, a second `Sink`)
    /// is rodio's supported pattern for concurrent playback. Opening a *second*
    /// `OutputStream` (as the standalone [`play_file_at`] does) is silent on some
    /// hosts — notably Windows/WASAPI, where the second stream opens without
    /// error yet never reaches the device. Use this whenever a synth stream
    /// already exists so the backing is actually audible alongside the synth.
    pub fn play_backing_at(
        &self,
        path: &std::path::Path,
        start: std::time::Duration,
    ) -> Result<BackingHandle, AudioError> {
        let sink = backing_sink(&self.stream_handle, path, start)?;
        Ok(BackingHandle {
            _stream: None,
            sink,
        })
    }
}

/// Resolve the SoundFont path: `$ROCKCRAFT_SF2` if set, else the default.
fn sf2_path() -> PathBuf {
    std::env::var_os(SF2_ENV)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(DEFAULT_SF2_PATH))
}

/// A playing backing-track audio file.
///
/// Beyond [`stop`](BackingHandle::stop), the track can be paused, resumed, and
/// re-seeked in place without tearing down the stream — used to hold the music
/// while waiting for the right notes and to scrub the track while editing.
/// Drop (or call [`stop`](BackingHandle::stop)) to stop playback; the device
/// stream is released on drop.
pub struct BackingHandle {
    // Keeps the device stream alive when this handle *owns* one (standalone
    // [`play_file_at`]). `None` when the backing shares the synth's stream via
    // [`AudioOut::play_backing_at`] — there the synth's `AudioOut` owns and
    // keeps the stream alive, so a second stream is neither held nor opened.
    _stream: Option<OutputStream>,
    sink: rodio::Sink,
}

impl BackingHandle {
    /// Stop playback immediately (also happens on drop).
    pub fn stop(&self) {
        self.sink.stop();
    }

    /// Pause playback, silencing output without dropping the stream. Idempotent;
    /// resume in place with [`resume`](BackingHandle::resume). Does not block —
    /// it only flags the sink.
    pub fn pause(&self) {
        self.sink.pause();
    }

    /// Resume playback from where [`pause`](BackingHandle::pause) left off.
    /// Idempotent; a no-op if not paused. Does not block.
    pub fn resume(&self) {
        self.sink.play();
    }

    /// Pause or resume in one call: `set_paused(true)` ==
    /// [`pause`](BackingHandle::pause), `set_paused(false)` ==
    /// [`resume`](BackingHandle::resume).
    pub fn set_paused(&self, paused: bool) {
        if paused {
            self.pause();
        } else {
            self.resume();
        }
    }

    /// Whether playback is currently paused.
    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// Jump to `pos` in the track, best-effort.
    ///
    /// Mirrors the seeking in [`play_file_at`]: if the decoder can't seek, the
    /// request is ignored rather than erroring. The paused/playing state is
    /// preserved across the seek. Does not block.
    pub fn seek(&self, pos: std::time::Duration) {
        // Best-effort: ignore decoders that don't support seeking.
        let _ = self.sink.try_seek(pos);
    }

    /// Set the playback speed multiplier (1.0 = normal). Resamples, so the pitch
    /// shifts with the speed — a slow-down practice mode, not time-stretch. Used
    /// to keep the backing audio in step with a slowed transport.
    pub fn set_speed(&self, speed: f32) {
        self.sink.set_speed(speed);
    }

    /// Set the backing track's level (M14-C) — the third fader next to the two
    /// synth buses, so the recording can sit under the notes or drop out
    /// entirely. Takes effect immediately and does not block.
    pub fn set_gain(&self, gain: Gain) {
        self.sink.set_volume(gain.value());
    }
}

/// Decode and start playing `path` on the default output device.
///
/// Returns immediately; playback runs on the rodio audio thread. Supports
/// wav, mp3, ogg, and flac via rodio/symphonia.
pub fn play_file(path: &std::path::Path) -> Result<BackingHandle, AudioError> {
    play_file_at(path, std::time::Duration::ZERO)
}

/// Like [`play_file`], but seek to `start` before playback begins.
///
/// Used to sync a backing track to the falling-note highway: the caller decides
/// the file position with `rockcraft_core::backing_position_us`. Seeking is
/// best-effort — if the decoder can't seek (some formats), playback falls back
/// to the start rather than failing, which is inaudible for the sub-frame
/// offsets this is normally called with.
pub fn play_file_at(
    path: &std::path::Path,
    start: std::time::Duration,
) -> Result<BackingHandle, AudioError> {
    let (stream, stream_handle) =
        OutputStream::try_default().map_err(|e| AudioError::Device(e.to_string()))?;
    let sink = backing_sink(&stream_handle, path, start)?;
    Ok(BackingHandle {
        _stream: Some(stream),
        sink,
    })
}

/// Build a paused-or-playing backing `Sink` on an existing output stream:
/// decode `path`, append it, and seek to `start` (best-effort). Shared by the
/// standalone [`play_file_at`] (own stream) and [`AudioOut::play_backing_at`]
/// (synth's stream).
fn backing_sink(
    stream_handle: &OutputStreamHandle,
    path: &std::path::Path,
    start: std::time::Duration,
) -> Result<rodio::Sink, AudioError> {
    let sink = rodio::Sink::try_new(stream_handle).map_err(|e| AudioError::Play(e.to_string()))?;
    let file = std::fs::File::open(path).map_err(AudioError::Io)?;
    let source = rodio::Decoder::new(std::io::BufReader::new(file))
        .map_err(|e| AudioError::Decode(e.to_string()))?;
    sink.append(source);
    if !start.is_zero() {
        // Best-effort: ignore decoders that don't support seeking.
        let _ = sink.try_seek(start);
    }
    Ok(sink)
}
