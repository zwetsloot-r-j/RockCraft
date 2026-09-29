//! Audio for RockCraft: playback, metronome, and MIDI synthesis.
//!
//! [`AudioOut`] opens the default output device and plays a SoundFont piano
//! synth; hand its [`SynthHandle`] the `NoteEvent`s coming off the piano and you
//! hear what you play. The synth machinery lives in [`synth`].
//!
//! [`play_file`] plays a decoded audio file (wav/mp3/ogg/flac) on a separate
//! output stream, returning a [`BackingHandle`] to stop, pause, resume, or seek
//! it; [`play_file_at`] starts it seeked to a position, for syncing a backing
//! track to the highway. Backing tracks are decoded fully into memory
//! ([`DecodedTrack`]) so they seek exactly in every format; long-lived callers
//! load once and play through [`AudioOut::play_backing_at`].
//!
//! Levels are per source (M14-C): the two synth buses take theirs through
//! [`SynthHandle::set_gain`], the backing track through
//! [`BackingHandle::set_gain`]. The settings themselves are `core::Mixer`.

mod fade;
pub mod synth;

pub use rockcraft_core as core;
pub use synth::{synth_from_sf2_bytes, SynthError, SynthHandle, SynthSource};

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

use fade::{steps_for, Ramp};

use rockcraft_core::Gain;
use rodio::{OutputStream, OutputStreamHandle, Source};

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

    /// Play a decoded backing track on the **same** output stream as the synth.
    ///
    /// Sharing the synth's `OutputStream` (one device stream, a second `Sink`)
    /// is rodio's supported pattern for concurrent playback. Opening a *second*
    /// `OutputStream` (as the standalone [`play_file_at`] does) is silent on some
    /// hosts — notably Windows/WASAPI, where the second stream opens without
    /// error yet never reaches the device. Use this whenever a synth stream
    /// already exists so the backing is actually audible alongside the synth.
    pub fn play_backing_at(
        &self,
        track: &DecodedTrack,
        start: std::time::Duration,
    ) -> Result<BackingHandle, AudioError> {
        self.backing_out().play_at(track, start)
    }

    /// A cloneable handle for starting backing tracks on this output stream
    /// from wherever the playback logic lives — the [`play_backing_at`]
    /// path without having to hold the (`!Send`, single-owner) `AudioOut`.
    ///
    /// [`play_backing_at`]: AudioOut::play_backing_at
    pub fn backing_out(&self) -> BackingOut {
        BackingOut {
            stream_handle: self.stream_handle.clone(),
        }
    }
}

/// Starts backing tracks on an [`AudioOut`]'s device stream (see
/// [`AudioOut::backing_out`]). Holds only a weak link to the stream: once the
/// `AudioOut` is dropped, [`play_at`](BackingOut::play_at) fails instead of
/// opening a new device.
#[derive(Clone)]
pub struct BackingOut {
    stream_handle: OutputStreamHandle,
}

impl BackingOut {
    /// Play `track` from `start` on the shared stream. Cheap: the samples are
    /// already decoded, so this never touches the disk or blocks.
    pub fn play_at(
        &self,
        track: &DecodedTrack,
        start: std::time::Duration,
    ) -> Result<BackingHandle, AudioError> {
        let (sink, fade) = backing_sink(&self.stream_handle, track, start)?;
        Ok(BackingHandle {
            _stream: None,
            sink,
            fade,
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
    fade: Arc<FadeCtl>,
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

    /// Fade the track out over `over`, then hold it silent in place — a soft
    /// [`pause`](BackingHandle::pause) (the wait-mode freeze). Undo with
    /// [`fade_in`](BackingHandle::fade_in); a re-[`seek`](BackingHandle::seek)
    /// first picks the position it resumes from.
    pub fn fade_out(&self, over: std::time::Duration) {
        self.fade.set(true, over);
    }

    /// Fade back in over `over` from a [`fade_out`](BackingHandle::fade_out),
    /// turning around mid-fade if need be. A no-op at full level.
    pub fn fade_in(&self, over: std::time::Duration) {
        self.fade.set(false, over);
    }

    /// Whether playback is currently paused.
    pub fn is_paused(&self) -> bool {
        self.sink.is_paused()
    }

    /// Whether the track has played to its end. A finished handle can't be
    /// sought back into; start a fresh one to play again.
    pub fn is_finished(&self) -> bool {
        self.sink.empty()
    }

    /// Where playback is in the file, as the sink last reported it (updated
    /// every few ms). It leads what is audible by the output buffer — a steady
    /// offset — and stalls when the device underruns, which is what
    /// `core::DriftGuard` watches for.
    pub fn position(&self) -> std::time::Duration {
        self.sink.get_pos()
    }

    /// Jump to `pos` in the track. Exact and instant for every format, since
    /// the track is decoded in memory ([`DecodedTrack`]); a `pos` past the end
    /// just runs out. The paused/playing state is preserved across the seek.
    pub fn seek(&self, pos: std::time::Duration) {
        // `TrackSource::try_seek` never fails; an error here only means the
        // sink has already finished, which leaves nothing to seek.
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
/// Returns once the file is decoded; playback runs on the rodio audio thread.
/// Supports wav, mp3, ogg, and flac.
pub fn play_file(path: &std::path::Path) -> Result<BackingHandle, AudioError> {
    play_file_at(path, std::time::Duration::ZERO)
}

/// Like [`play_file`], but start at `start` in the file.
///
/// Used to sync a backing track to the falling-note highway: the caller decides
/// the file position with `rockcraft_core::backing_position_us`. Decodes the
/// whole file first ([`DecodedTrack::load`]); a caller that plays the same file
/// repeatedly should load it once and use [`AudioOut::play_backing_at`].
pub fn play_file_at(
    path: &std::path::Path,
    start: std::time::Duration,
) -> Result<BackingHandle, AudioError> {
    let track = DecodedTrack::load(path)?;
    let (stream, stream_handle) =
        OutputStream::try_default().map_err(|e| AudioError::Device(e.to_string()))?;
    let (sink, fade) = backing_sink(&stream_handle, &track, start)?;
    Ok(BackingHandle {
        _stream: Some(stream),
        sink,
        fade,
    })
}

/// Build a playing backing `Sink` on an existing output stream, positioned at
/// `start`. Shared by the standalone [`play_file_at`] (own stream) and
/// [`AudioOut::play_backing_at`] (synth's stream).
fn backing_sink(
    stream_handle: &OutputStreamHandle,
    track: &DecodedTrack,
    start: std::time::Duration,
) -> Result<(rodio::Sink, Arc<FadeCtl>), AudioError> {
    let sink = rodio::Sink::try_new(stream_handle).map_err(|e| AudioError::Play(e.to_string()))?;
    let mut source = track.source();
    let fade = source.fade.clone();
    // Position the source before it reaches the sink: a sink-level seek is only
    // applied once the device pulls samples, so the first few ms would play
    // from the top.
    source.seek_to(start);
    sink.append(source);
    Ok((sink, fade))
}

/// A backing-track audio file decoded fully into memory.
///
/// Streaming decoders can't be trusted to seek: rodio 0.20's Vorbis and FLAC
/// decoders don't support it at all, so a backing `.ogg` ignored every seek and
/// kept playing from wherever it was instead of following the playhead. Holding
/// the samples makes every seek exact and instant, for every format, and keeps
/// decoding off the real-time audio thread. Load it once (it can take a moment
/// for a long track) and play it any number of times; clones share the samples.
#[derive(Clone)]
pub struct DecodedTrack {
    channels: u16,
    sample_rate: u32,
    samples: Arc<[i16]>,
}

impl DecodedTrack {
    /// Decode the audio file at `path` (wav, mp3, ogg, or flac).
    pub fn load(path: &std::path::Path) -> Result<Self, AudioError> {
        let file = std::fs::File::open(path).map_err(AudioError::Io)?;
        let decoder = rodio::Decoder::new(std::io::BufReader::new(file))
            .map_err(|e| AudioError::Decode(e.to_string()))?;
        let channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        if channels == 0 || sample_rate == 0 {
            return Err(AudioError::Decode(format!(
                "{}: no audio ({channels} channels at {sample_rate} Hz)",
                path.display()
            )));
        }
        Ok(Self {
            channels,
            sample_rate,
            samples: decoder.collect(),
        })
    }

    /// Decode the file at `path` on a background thread. A full song takes a
    /// noticeable moment to decode; poll the returned [`TrackLoader`] from the
    /// app loop instead of stalling it.
    pub fn load_in_background(path: &std::path::Path) -> TrackLoader {
        let (tx, rx) = std::sync::mpsc::channel();
        let path = path.to_path_buf();
        let spawned = std::thread::Builder::new()
            .name("rockcraft-decode".into())
            .spawn(move || {
                let _ = tx.send(DecodedTrack::load(&path));
            });
        TrackLoader {
            state: match spawned {
                Ok(_) => LoadState::Loading(rx),
                Err(e) => LoadState::Failed(AudioError::Io(e)),
            },
        }
    }

    /// Length of the track.
    pub fn duration(&self) -> std::time::Duration {
        let frames = self.samples.len() / self.channels as usize;
        std::time::Duration::from_secs_f64(frames as f64 / self.sample_rate as f64)
    }

    fn source(&self) -> TrackSource {
        TrackSource {
            track: self.clone(),
            pos: 0,
            fade: Arc::default(),
            ramp: Ramp::FULL,
            emitted: 0,
        }
    }
}

/// A [`DecodedTrack`] decoding on a background thread
/// ([`DecodedTrack::load_in_background`]). Poll it once per app-loop tick.
pub struct TrackLoader {
    state: LoadState,
}

enum LoadState {
    Loading(std::sync::mpsc::Receiver<Result<DecodedTrack, AudioError>>),
    Ready(DecodedTrack),
    Failed(AudioError),
}

/// Where a [`TrackLoader`] has got to.
pub enum TrackStatus<'a> {
    /// Still decoding.
    Loading,
    /// Decoded and ready to play.
    Ready(&'a DecodedTrack),
    /// The file could not be read or decoded.
    Failed(&'a AudioError),
}

impl TrackLoader {
    /// Check on the decode without blocking.
    pub fn poll(&mut self) -> TrackStatus<'_> {
        if let LoadState::Loading(rx) = &self.state {
            match rx.try_recv() {
                Ok(Ok(track)) => self.state = LoadState::Ready(track),
                Ok(Err(e)) => self.state = LoadState::Failed(e),
                Err(std::sync::mpsc::TryRecvError::Empty) => return TrackStatus::Loading,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.state = LoadState::Failed(AudioError::Decode(
                        "decoder thread exited without a result".into(),
                    ))
                }
            }
        }
        match &self.state {
            LoadState::Loading(_) => TrackStatus::Loading,
            LoadState::Ready(t) => TrackStatus::Ready(t),
            LoadState::Failed(e) => TrackStatus::Failed(e),
        }
    }
}

/// The app side's say over a playing track's fade: which way, and how fast.
/// Read by the audio thread once per frame; lock-free.
#[derive(Default)]
struct FadeCtl {
    out: AtomicBool,
    ms: AtomicU32,
}

impl FadeCtl {
    fn set(&self, out: bool, over: std::time::Duration) {
        let ms = over.as_millis().min(u32::MAX as u128) as u32;
        self.ms.store(ms, Ordering::Relaxed);
        self.out.store(out, Ordering::Relaxed);
    }
}

/// A playing cursor over a [`DecodedTrack`]; seeking just moves `pos`.
struct TrackSource {
    track: DecodedTrack,
    /// Index of the next sample (interleaved), always on a frame boundary
    /// after a seek.
    pos: usize,
    fade: Arc<FadeCtl>,
    /// The fade as applied, stepped once per frame.
    ramp: Ramp,
    /// Samples handed out, silence included — frames are counted off this, so
    /// the channels stay in step even while silent samples stand in for audio.
    emitted: u64,
}

impl TrackSource {
    fn seek_to(&mut self, pos: std::time::Duration) {
        let frame = (pos.as_secs_f64() * self.track.sample_rate as f64).round() as usize;
        let sample = frame.saturating_mul(self.track.channels as usize);
        self.pos = sample.min(self.track.samples.len());
    }
}

impl TrackSource {
    /// Once per frame: pick up a new fade request and step the ramp.
    fn step_fade(&mut self) {
        let out = self.fade.out.load(Ordering::Relaxed);
        if out != self.ramp.is_fading_out() {
            let ms = self.fade.ms.load(Ordering::Relaxed);
            self.ramp
                .fade(out, steps_for(ms, self.track.sample_rate as f64));
        }
        if !self.ramp.is_steady() {
            self.ramp.tick();
        }
    }
}

impl Iterator for TrackSource {
    type Item = i16;

    fn next(&mut self) -> Option<i16> {
        if self.emitted.is_multiple_of(self.track.channels as u64) {
            self.step_fade();
        }
        self.emitted += 1;
        // Faded out: hold the position and play silence until faded back in.
        if self.ramp.is_silent() {
            return Some(0);
        }
        let sample = self.track.samples.get(self.pos).copied()?;
        self.pos += 1;
        Some((sample as f32 * self.ramp.level()) as i16)
    }
}

impl Source for TrackSource {
    fn current_frame_len(&self) -> Option<usize> {
        // Channels and rate never change mid-track. (Not the remaining sample
        // count: while faded out the source plays silence without advancing.)
        None
    }

    fn channels(&self) -> u16 {
        self.track.channels
    }

    fn sample_rate(&self) -> u32 {
        self.track.sample_rate
    }

    fn total_duration(&self) -> Option<std::time::Duration> {
        Some(self.track.duration())
    }

    fn try_seek(&mut self, pos: std::time::Duration) -> Result<(), rodio::source::SeekError> {
        self.seek_to(pos);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Write `secs` of 8 kHz mono 16-bit PCM WAV — a ramp, so every sample
    /// tells where it came from — into a per-test temp file. Generated rather
    /// than committed: the repo keeps no audio in git (`check-no-media.sh`).
    fn ramp_wav(name: &str, secs: u32) -> PathBuf {
        let samples: Vec<i16> = (0..8_000 * secs).map(|i| (i % 30_000) as i16).collect();
        let data_len = samples.len() as u32 * 2;
        let mut wav = Vec::with_capacity(44 + data_len as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data_len).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes()); // fmt chunk size
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&8_000u32.to_le_bytes()); // sample rate
        wav.extend_from_slice(&16_000u32.to_le_bytes()); // byte rate
        wav.extend_from_slice(&2u16.to_le_bytes()); // block align
        wav.extend_from_slice(&16u16.to_le_bytes()); // bits per sample
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data_len.to_le_bytes());
        for s in samples {
            wav.extend_from_slice(&s.to_le_bytes());
        }
        let path =
            std::env::temp_dir().join(format!("rockcraft-audio-{}-{name}.wav", std::process::id()));
        std::fs::write(&path, wav).unwrap();
        path
    }

    /// Regression: rodio 0.20 can't seek Vorbis/FLAC streams, so an imported
    /// `backing.ogg` ignored every seek and drifted from the playhead. A decoded
    /// track seeks in memory — exactly, and independent of the file format.
    /// Poll a loader until it settles (the decode runs on its own thread).
    fn settle(loader: &mut TrackLoader) -> Result<std::time::Duration, String> {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            match loader.poll() {
                TrackStatus::Loading if std::time::Instant::now() < deadline => {
                    std::thread::sleep(std::time::Duration::from_millis(5))
                }
                TrackStatus::Loading => return Err("timed out".into()),
                TrackStatus::Ready(t) => return Ok(t.duration()),
                TrackStatus::Failed(e) => return Err(e.to_string()),
            }
        }
    }

    #[test]
    fn a_background_load_becomes_ready() {
        let path = ramp_wav("bg", 2);
        let mut loader = DecodedTrack::load_in_background(&path);
        assert_eq!(settle(&mut loader), Ok(std::time::Duration::from_secs(2)));
        // Settled stays settled.
        assert!(matches!(loader.poll(), TrackStatus::Ready(_)));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn a_background_load_of_a_missing_file_fails() {
        let mut loader =
            DecodedTrack::load_in_background(std::path::Path::new("/definitely/not/here.wav"));
        assert!(settle(&mut loader).is_err());
    }

    #[test]
    fn a_decoded_track_seeks_exactly() {
        let path = ramp_wav("seek", 3);
        let track = DecodedTrack::load(&path).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!((track.channels, track.sample_rate), (1, 8_000));
        assert_eq!(track.duration(), Duration::from_secs(3));
        let mut src = track.source();
        src.try_seek(Duration::from_secs(2)).unwrap();
        assert_eq!(src.next(), Some(16_000)); // sample 2 s × 8 kHz
        assert_eq!(src.count(), 8_000 - 1);
    }

    /// Seeking backwards — restarting playback — rewinds to the top.
    #[test]
    fn seeking_back_rewinds() {
        let path = ramp_wav("rewind", 3);
        let track = DecodedTrack::load(&path).unwrap();
        std::fs::remove_file(&path).ok();
        let mut src = track.source();
        src.try_seek(Duration::from_millis(2_500)).unwrap();
        src.try_seek(Duration::ZERO).unwrap();
        assert_eq!(src.next(), Some(0));
        assert_eq!(src.count(), track.samples.len() - 1);
    }

    /// A seek past the end, or mid-frame, stays in range and frame-aligned.
    #[test]
    fn seek_clamps_to_the_track() {
        let track = DecodedTrack {
            channels: 2,
            sample_rate: 10,
            samples: vec![0; 40].into(),
        };
        assert_eq!(track.duration(), Duration::from_secs(2));
        let mut src = track.source();
        src.seek_to(Duration::from_secs(60));
        assert_eq!(src.next(), None);
        src.seek_to(Duration::from_millis(1_049)); // frame 10 → sample 20
        assert_eq!(src.pos, 20);
    }

    /// A fade-out ramps down to silence and then holds the position; fading
    /// back in resumes from exactly there.
    #[test]
    fn a_faded_out_track_holds_its_place() {
        let track = DecodedTrack {
            channels: 2,
            sample_rate: 1_000,
            samples: vec![1_000; 2_000].into(),
        };
        let mut src = track.source();
        src.fade.set(true, Duration::from_millis(4)); // 4 frames
        let out: Vec<i16> = src.by_ref().take(12).collect();
        // Both channels of a frame share a level: 0.75, 0.5, 0.25, then silence.
        assert_eq!(out, [750, 750, 500, 500, 250, 250, 0, 0, 0, 0, 0, 0]);
        let parked = src.pos;
        assert_eq!(parked, 6, "3 audible frames consumed, then held");
        assert_eq!(src.by_ref().take(100).filter(|&s| s != 0).count(), 0);
        assert_eq!(src.pos, parked);
        src.fade.set(false, Duration::from_millis(2));
        let back: Vec<i16> = src.by_ref().take(6).collect();
        assert_eq!(back, [500, 500, 1_000, 1_000, 1_000, 1_000]);
    }

    #[test]
    fn load_reports_a_missing_file() {
        assert!(matches!(
            DecodedTrack::load(std::path::Path::new("no/such/file.ogg")),
            Err(AudioError::Io(_))
        ));
    }
}
