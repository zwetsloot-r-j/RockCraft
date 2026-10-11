//! Backing-track waveform analysis (M21-A): two per-slice curves the editor
//! draws behind the grid so spikes in the real music guide rhythm editing.
//!
//! - **Envelope** — loudness: the peak level per slice on a 48 dB scale.
//! - **Onsets** — how sharply the sound *rises* per slice (band-wise energy
//!   flux). It peaks where notes and hits begin, so it reads better than the
//!   envelope for rhythm, which a mix's kick and bass otherwise dominate.
//!
//! Pure: it takes decoded samples, never a file. Decoding is the `audio`
//! crate's job. No FFT — three one-pole band filters are enough for a guide.

use serde::{Deserialize, Serialize};

/// Width of one slice. 10 ms: blocky at deep zoom, which is accepted for v1.
pub const WAVEFORM_BUCKET_US: u64 = 10_000;

/// Range the envelope's dB scale spans: `-ENVELOPE_RANGE_DB` (relative to the
/// loudest slice) maps to 0, the loudest slice to 255.
const ENVELOPE_RANGE_DB: f64 = 48.0;

/// Band edges (Hz) for the onset curve: low < 200, mid 200–2000, high > 2000.
const LOW_MID_HZ: f64 = 200.0;
const MID_HIGH_HZ: f64 = 2_000.0;

/// Compression applied to a band's RMS (`0..=1`) before differencing:
/// `ln(1 + ONSET_COMPRESSION · rms)`. Log-like for loud material (so a hit in a
/// loud passage still registers) but near-linear for quiet noise, so hiss and
/// reverb tails don't read as onsets.
const ONSET_COMPRESSION: f64 = 100.0;

/// Percentile of the non-zero flux values that maps to 255 (louder clamp).
const ONSET_NORM_PERCENTILE: f64 = 0.99;

/// A backing track's waveform, sliced into `bucket_us`-wide buckets.
///
/// Bucket `i` covers **backing-file** position `[i·bucket_us, (i+1)·bucket_us)`,
/// not song time: song time `t` is file position `t + backing_offset_us`, so a
/// change of the backing offset never needs a re-analysis.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Waveform {
    pub bucket_us: u64,
    /// Loudness per bucket, `0..=255`.
    pub envelope: Vec<u8>,
    /// Onset strength per bucket, `0..=255`; same length as `envelope`.
    pub onsets: Vec<u8>,
}

/// Analyse interleaved i16 `samples` (as `DecodedTrack` holds them) into a
/// [`Waveform`] of `bucket_us`-wide buckets. The last bucket may be partial and
/// is included. Empty input (or a zero channel count / rate / bucket) gives
/// empty curves; silence gives all zeros.
pub fn analyze(samples: &[i16], channels: u16, sample_rate: u32, bucket_us: u64) -> Waveform {
    let empty = Waveform {
        bucket_us,
        envelope: Vec::new(),
        onsets: Vec::new(),
    };
    if channels == 0 || sample_rate == 0 || bucket_us == 0 {
        return empty;
    }
    let ch = channels as usize;
    let frames = samples.len() / ch;
    if frames == 0 {
        return empty;
    }
    // Frames per bucket, at least one (a bucket shorter than a frame is moot).
    let per_bucket = ((sample_rate as u64 * bucket_us) / 1_000_000).max(1) as usize;
    let buckets = frames.div_ceil(per_bucket);

    let a_low = one_pole_coeff(LOW_MID_HZ, sample_rate);
    let a_high = one_pole_coeff(MID_HIGH_HZ, sample_rate);
    let (mut lp_low, mut lp_high) = (0.0f64, 0.0f64);

    let mut peaks = Vec::with_capacity(buckets);
    let mut band_energy: Vec<[f64; 3]> = Vec::with_capacity(buckets);
    for b in 0..buckets {
        let lo = b * per_bucket;
        let hi = ((b + 1) * per_bucket).min(frames);
        let mut peak = 0.0f64;
        let mut sq = [0.0f64; 3];
        for f in lo..hi {
            let frame = &samples[f * ch..(f + 1) * ch];
            let x = frame.iter().map(|&s| s as f64).sum::<f64>() / (ch as f64 * 32_768.0);
            peak = peak.max(x.abs());
            lp_low += a_low * (x - lp_low);
            lp_high += a_high * (x - lp_high);
            let bands = [lp_low, lp_high - lp_low, x - lp_high];
            for (acc, v) in sq.iter_mut().zip(bands) {
                *acc += v * v;
            }
        }
        let n = (hi - lo) as f64;
        peaks.push(peak);
        band_energy.push(sq.map(|s| (1.0 + ONSET_COMPRESSION * (s / n).sqrt()).ln()));
    }

    Waveform {
        bucket_us,
        envelope: envelope_of(&peaks),
        onsets: onsets_of(&band_energy),
    }
}

/// One-pole low-pass smoothing coefficient for cutoff `hz` at `sample_rate`.
fn one_pole_coeff(hz: f64, sample_rate: u32) -> f64 {
    1.0 - (-2.0 * std::f64::consts::PI * hz / sample_rate as f64).exp()
}

/// Peaks → `0..=255` on a dB scale relative to the loudest bucket.
fn envelope_of(peaks: &[f64]) -> Vec<u8> {
    let max = peaks.iter().copied().fold(0.0f64, f64::max);
    if max <= 0.0 {
        return vec![0; peaks.len()];
    }
    peaks
        .iter()
        .map(|&p| {
            if p <= 0.0 {
                return 0;
            }
            let db = 20.0 * (p / max).log10();
            to_u8((1.0 + db / ENVELOPE_RANGE_DB) * 255.0)
        })
        .collect()
}

/// Band energies → onset strength `0..=255`: the summed rise of each band's
/// compressed level over the previous bucket, normalised to a high percentile.
fn onsets_of(energy: &[[f64; 3]]) -> Vec<u8> {
    let flux: Vec<f64> = (0..energy.len())
        .map(|i| match i {
            0 => 0.0,
            _ => (0..3)
                .map(|b| (energy[i][b] - energy[i - 1][b]).max(0.0))
                .sum(),
        })
        .collect();
    let mut nonzero: Vec<f64> = flux.iter().copied().filter(|&f| f > 0.0).collect();
    if nonzero.is_empty() {
        return vec![0; flux.len()];
    }
    nonzero.sort_by(f64::total_cmp);
    let idx = ((nonzero.len() - 1) as f64 * ONSET_NORM_PERCENTILE).round() as usize;
    let norm = nonzero[idx];
    flux.iter().map(|&f| to_u8(f / norm * 255.0)).collect()
}

fn to_u8(v: f64) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 44_100;

    fn tone(secs: f64, amp: f64) -> Vec<i16> {
        let n = (secs * SR as f64) as usize;
        (0..n)
            .map(|i| {
                let t = i as f64 / SR as f64;
                (amp * 32_767.0 * (2.0 * std::f64::consts::PI * 440.0 * t).sin()) as i16
            })
            .collect()
    }

    #[test]
    fn silence_is_all_zero_with_one_bucket_per_slice() {
        let w = analyze(&vec![0; SR as usize], 1, SR, WAVEFORM_BUCKET_US);
        assert_eq!(w.envelope.len(), 100);
        assert_eq!(w.onsets.len(), 100);
        assert!(w.envelope.iter().all(|&v| v == 0));
        assert!(w.onsets.iter().all(|&v| v == 0));
    }

    #[test]
    fn full_scale_tone_is_max_and_minus_24_db_is_about_half() {
        let mut s = tone(0.5, 1.0);
        s.extend(tone(0.5, 10f64.powf(-24.0 / 20.0)));
        let w = analyze(&s, 1, SR, WAVEFORM_BUCKET_US);
        // Skip the edges of each half (partial cycles / the step itself).
        assert!(
            w.envelope[2..48].iter().all(|&v| v >= 253),
            "{:?}",
            &w.envelope[..50]
        );
        for &v in &w.envelope[52..98] {
            assert!((124..=131).contains(&v), "{v}");
        }
    }

    #[test]
    fn clicks_peak_the_onset_curve_at_their_buckets() {
        let mut s = vec![0i16; 3 * SR as usize];
        let click_times_s = [0.5, 1.0, 1.5, 2.0, 2.5];
        for &t in &click_times_s {
            let at = (t * SR as f64) as usize;
            for (k, v) in s[at..at + 200].iter_mut().enumerate() {
                *v = if k % 2 == 0 { 30_000 } else { -30_000 };
            }
        }
        let w = analyze(&s, 1, SR, WAVEFORM_BUCKET_US);
        for &t in &click_times_s {
            let b = (t * 1_000_000.0) as usize / WAVEFORM_BUCKET_US as usize;
            let peak = (b.saturating_sub(1)..=b + 1)
                .map(|i| w.onsets[i])
                .max()
                .unwrap();
            assert!(peak >= 200, "click at {t}s: {peak}");
        }
        let near_click = |i: usize| {
            click_times_s.iter().any(|&t| {
                let b = (t * 1_000_000.0) as usize / WAVEFORM_BUCKET_US as usize;
                i + 1 >= b && i <= b + 1
            })
        };
        for (i, &v) in w.onsets.iter().enumerate() {
            if !near_click(i) {
                assert!(v <= 20, "bucket {i}: {v}");
            }
        }
    }

    #[test]
    fn stereo_with_a_silent_channel_reads_6_db_down() {
        let mono = tone(0.5, 1.0);
        let mut stereo = Vec::with_capacity(mono.len() * 2);
        for &x in &mono {
            stereo.push(x);
            stereo.push(0);
        }
        // A loud reference bucket so the stereo level is relative, not 255.
        let mut s = vec![32_767i16, 32_767];
        s.extend(stereo);
        let w = analyze(&s, 2, SR, WAVEFORM_BUCKET_US);
        // -6 dB on a 48 dB scale ≈ 255 · (1 − 6/48) ≈ 223.
        for &v in &w.envelope[2..48] {
            assert!((219..=227).contains(&v), "{v}");
        }
    }

    #[test]
    fn partial_last_bucket_is_included_and_empty_input_is_empty() {
        let w = analyze(&tone(0.015, 0.5), 1, SR, WAVEFORM_BUCKET_US);
        assert_eq!(w.envelope.len(), 2);
        assert_eq!(w.onsets.len(), 2);
        let e = analyze(&[], 2, SR, WAVEFORM_BUCKET_US);
        assert!(e.envelope.is_empty() && e.onsets.is_empty());
    }
}
