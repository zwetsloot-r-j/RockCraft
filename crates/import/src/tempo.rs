//! Tempo-map detection from audio (M16-A): run the `tools/tempo-map` sidecar
//! over a piece's backing audio and return its per-bar downbeats.
//!
//! Same subprocess contract as the other sidecars: `python3 <script> --in X
//! --out -`, stdout is JSON, stderr is diagnostics. Times here are **file** times
//! (µs into the audio file); [`file_to_song_us`] / [`song_to_file_us`] convert
//! with a backing's `audio_start_us`.

use std::path::{Path, PathBuf};
use std::process::Command;

use serde::{Deserialize, Serialize};

use crate::pipeline::{env_ffmpeg_cmd, workspace_root};
use crate::ImportError;

/// Options for [`detect_tempo_map`].
#[derive(Debug, Clone, PartialEq)]
pub struct TempoMapOpts {
    /// Beats grouped into each bar (the piece's metre).
    pub beats_per_bar: u8,
    /// A known downbeat (file µs); the detector snaps it to the nearest beat.
    /// `None` lets the detector pick the bar phase from bass accents.
    pub anchor_file_us: Option<u64>,
    /// The expected beat tempo; resolves the half/double-time ambiguity.
    pub tempo_hint_bpm: Option<f64>,
}

/// The sidecar's result. All times are file µs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DetectedTempoMap {
    /// Every bar's downbeat, ascending; the last entry closes the final bar.
    pub bars_us: Vec<u64>,
    /// Every detected beat.
    pub beats_us: Vec<u64>,
    /// Median beat tempo.
    pub bpm: f64,
    /// The tracker's raw pulse rate before folding to beats.
    pub pulse_bpm: f64,
    /// The downbeat the bars were grouped from.
    pub anchor_us: u64,
}

/// Detect a per-bar tempo map from `audio`. A non-WAV input is first decoded
/// to a temporary WAV with ffmpeg.
pub fn detect_tempo_map(
    audio: &Path,
    opts: &TempoMapOpts,
) -> Result<DetectedTempoMap, ImportError> {
    let script = find_tempo_sidecar(&workspace_root())?;
    let is_wav = audio
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("wav"));
    let temp = if is_wav {
        None
    } else {
        Some(decode_to_wav(audio)?)
    };
    let input = temp.as_deref().unwrap_or(audio);

    let mut cmd = Command::new("python3");
    cmd.arg(&script)
        .arg("--in")
        .arg(input)
        .arg("--out")
        .arg("-")
        .arg("--beats-per-bar")
        .arg(opts.beats_per_bar.max(1).to_string());
    if let Some(a) = opts.anchor_file_us {
        cmd.arg("--anchor-us").arg(a.to_string());
    }
    if let Some(h) = opts.tempo_hint_bpm {
        cmd.arg("--tempo-hint").arg(h.to_string());
    }
    let output = cmd.output();
    if let Some(t) = &temp {
        let _ = std::fs::remove_file(t);
    }
    let output = output.map_err(|e| {
        ImportError::SidecarMissing(format!(
            "could not launch python3: {e}; install python3 with numpy \
             (see tools/tempo-map/requirements.txt)"
        ))
    })?;
    if !output.status.success() {
        return Err(ImportError::SidecarFailed(
            String::from_utf8_lossy(&output.stderr).into_owned(),
        ));
    }
    parse_tempo_map(&String::from_utf8_lossy(&output.stdout))
}

/// Parse the sidecar's JSON, rejecting a map that isn't strictly ascending.
pub fn parse_tempo_map(json: &str) -> Result<DetectedTempoMap, ImportError> {
    let map: DetectedTempoMap =
        serde_json::from_str(json).map_err(|e| ImportError::Json(e.to_string()))?;
    if map.bars_us.windows(2).any(|w| w[1] <= w[0]) {
        return Err(ImportError::SidecarFailed(
            "tempo map bars are not strictly ascending".into(),
        ));
    }
    Ok(map)
}

/// File time → song time for a backing whose file position `audio_start_us`
/// lines up with song time 0 (`song = file − audio_start_us`). `None` for a
/// file position that falls before song time 0.
pub fn file_to_song_us(file_us: u64, audio_start_us: i64) -> Option<u64> {
    let song = file_us as i128 - audio_start_us as i128;
    u64::try_from(song).ok()
}

/// Song time → file time (`file = song + audio_start_us`); `None` before the
/// file's start (a negative offset's silent lead-in).
pub fn song_to_file_us(song_us: u64, audio_start_us: i64) -> Option<u64> {
    let file = song_us as i128 + audio_start_us as i128;
    u64::try_from(file).ok()
}

fn find_tempo_sidecar(workspace: &Path) -> Result<PathBuf, ImportError> {
    let script = workspace.join("tools/tempo-map/tempo_map.py");
    if script.exists() {
        Ok(script)
    } else {
        Err(ImportError::SidecarMissing(format!(
            "tempo-map sidecar not found at {}",
            script.display()
        )))
    }
}

fn decode_to_wav(audio: &Path) -> Result<PathBuf, ImportError> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let out = std::env::temp_dir().join(format!("rockcraft-tempo-{stamp}.wav"));
    let status = Command::new(env_ffmpeg_cmd())
        .args(["-y", "-v", "error", "-i"])
        .arg(audio)
        .args(["-vn", "-ac", "1", "-ar", "22050"])
        .arg(&out)
        .stdin(std::process::Stdio::null())
        .status()
        .map_err(|e| ImportError::Io(format!("could not launch ffmpeg: {e}")))?;
    if !status.success() {
        let _ = std::fs::remove_file(&out);
        return Err(ImportError::Io(format!(
            "ffmpeg could not decode {} to WAV ({status})",
            audio.display()
        )));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_song_conversion_honours_the_backing_offset() {
        // Positive offset: the file starts before song time 0.
        assert_eq!(file_to_song_us(3_000_000, 1_000_000), Some(2_000_000));
        assert_eq!(file_to_song_us(500_000, 1_000_000), None);
        assert_eq!(song_to_file_us(2_000_000, 1_000_000), Some(3_000_000));
        // Negative offset: a silent lead-in before the file starts.
        assert_eq!(file_to_song_us(0, -500_000), Some(500_000));
        assert_eq!(song_to_file_us(200_000, -500_000), None);
        assert_eq!(song_to_file_us(700_000, -500_000), Some(200_000));
    }

    #[test]
    fn parses_sidecar_json_and_rejects_unordered_bars() {
        let ok = r#"{"version":1,"beats_us":[0,500000,1000000],"bars_us":[0,2000000],
                    "bpm":120.0,"pulse_bpm":120.0,"anchor_us":0}"#;
        let m = parse_tempo_map(ok).unwrap();
        assert_eq!(m.bars_us, vec![0, 2_000_000]);
        assert_eq!(m.bpm, 120.0);
        let bad = ok.replace("[0,2000000]", "[2000000,0]");
        assert!(parse_tempo_map(&bad).is_err());
        assert!(parse_tempo_map("not json").is_err());
    }

    #[test]
    fn missing_sidecar_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(
            find_tempo_sidecar(dir.path()),
            Err(ImportError::SidecarMissing(_))
        ));
    }
}
