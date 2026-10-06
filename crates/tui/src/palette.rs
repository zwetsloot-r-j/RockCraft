//! Highway note colours — pure, no terminal I/O.
//!
//! Mirrors the desktop highway's colour modes (`HighwayCanvas.ts`): colour a
//! note by the **hand** that plays it (the default), by its **pitch class**
//! around a 12-step colour wheel, or in one **accent** colour. The palette
//! values are the desktop's, so a piece reads the same in both frontends.
//!
//! On top of the mode, two cues layer on every note: a note **sounding now** is
//! lifted brighter, and a **black-key** note is dimmed (with the shaded glyph
//! from `play::note_style`) so accidentals stay distinct in any mode.

use ratatui::style::Color;
use rockcraft_core::Hand;

/// How highway notes are coloured. Cycled with `c` on the play screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColorMode {
    /// Left hand teal, right hand amber (the desktop default).
    #[default]
    Hands,
    /// One hue per pitch class, round the colour wheel.
    Spectrum,
    /// Every note in one accent colour.
    Accent,
}

impl ColorMode {
    /// The next mode in the `c` cycle: hands → spectrum → accent → hands.
    pub const fn next(self) -> Self {
        match self {
            ColorMode::Hands => ColorMode::Spectrum,
            ColorMode::Spectrum => ColorMode::Accent,
            ColorMode::Accent => ColorMode::Hands,
        }
    }

    /// Short label for the status line.
    pub const fn label(self) -> &'static str {
        match self {
            ColorMode::Hands => "hands",
            ColorMode::Spectrum => "spectrum",
            ColorMode::Accent => "accent",
        }
    }
}

/// An sRGB colour as 8-bit channels; converted to a terminal colour at the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    /// Linear mix toward `other` by `t` (`0.0` = self, `1.0` = other).
    pub fn mix(self, other: Rgb, t: f32) -> Rgb {
        let ch = |a: u8, b: u8| (a as f32 + (b as f32 - a as f32) * t).round() as u8;
        Rgb(
            ch(self.0, other.0),
            ch(self.1, other.1),
            ch(self.2, other.2),
        )
    }
}

impl From<Rgb> for Color {
    fn from(c: Rgb) -> Self {
        Color::Rgb(c.0, c.1, c.2)
    }
}

/// Left-hand note colour (desktop `handColors.L`).
pub const LEFT_HAND: Rgb = Rgb(0x5a, 0xd1, 0xc7);
/// Right-hand note colour (desktop `handColors.R`).
pub const RIGHT_HAND: Rgb = Rgb(0xe6, 0xa1, 0x4b);
/// The single accent colour (desktop `accent`).
pub const ACCENT: Rgb = Rgb(0x7a, 0xa2, 0xff);
/// The highway background the dimming mixes toward (desktop `bg`).
pub const BACKGROUND: Rgb = Rgb(0x0f, 0x10, 0x16);
const WHITE: Rgb = Rgb(0xff, 0xff, 0xff);

/// How far a sounding note is lifted toward white (hands / accent modes).
const ACTIVE_LIFT: f32 = 0.4;
/// How far a black-key note is sunk toward the background.
const BLACK_KEY_DIM: f32 = 0.35;

/// The hue (degrees) of a pitch class on the spectrum wheel — the desktop's
/// `spectrumHue`, so C, C♯, … land on the same colours.
pub fn spectrum_hue(note: u8) -> f32 {
    ((note % 12) as f32 * 30.0 + 8.0) % 360.0
}

/// The OKLCH lightness, chroma and hue of a note on the spectrum wheel, from a
/// base lightness and chroma. C and B are neighbours on the wheel (8° vs 338°)
/// and read alike, so they are pulled apart: C to a deeper, clearer red, B to a
/// softer pink. Mirrors the desktop's `spectrumTone` (`utils.ts`).
pub fn spectrum_tone(note: u8, l: f32, c: f32) -> (f32, f32, f32) {
    match note % 12 {
        0 => ((l - 0.12).clamp(0.0, 1.0), c + 0.03, 25.0),
        11 => ((l + 0.10).clamp(0.0, 1.0), c * 0.5, 350.0),
        _ => (l, c, spectrum_hue(note)),
    }
}

/// The colour of a highway note: by `mode`, lifted when `active` (sounding
/// now), dimmed for a black key.
pub fn note_color(mode: ColorMode, note: u8, hand: Hand, active: bool, black_key: bool) -> Rgb {
    let base = match mode {
        // The desktop lifts spectrum notes in lightness, not toward white, so
        // the hue stays saturated.
        ColorMode::Spectrum => {
            let l = if active { 0.82 } else { 0.70 };
            let (l, c, h) = spectrum_tone(note, l, 0.16);
            oklch_to_rgb(l, c, h)
        }
        ColorMode::Hands | ColorMode::Accent => {
            let c = match (mode, hand) {
                (ColorMode::Accent, _) => ACCENT,
                (_, Hand::Left) => LEFT_HAND,
                (_, Hand::Right) => RIGHT_HAND,
            };
            if active {
                c.mix(WHITE, ACTIVE_LIFT)
            } else {
                c
            }
        }
    };
    if black_key {
        base.mix(BACKGROUND, BLACK_KEY_DIM)
    } else {
        base
    }
}

/// Convert an OKLCH colour (the desktop's CSS `oklch(l c h)`) to 8-bit sRGB,
/// clamping anything outside the sRGB gamut.
pub fn oklch_to_rgb(l: f32, c: f32, hue_deg: f32) -> Rgb {
    let h = hue_deg.to_radians();
    let (a, b) = (c * h.cos(), c * h.sin());
    // OKLab → linear sRGB (Björn Ottosson's reference matrices).
    let l_ = l + 0.396_337_78 * a + 0.215_803_76 * b;
    let m_ = l - 0.105_561_346 * a - 0.063_854_17 * b;
    let s_ = l - 0.089_484_18 * a - 1.291_485_5 * b;
    let (l3, m3, s3) = (l_ * l_ * l_, m_ * m_ * m_, s_ * s_ * s_);
    let r = 4.076_741_7 * l3 - 3.307_711_6 * m3 + 0.230_969_94 * s3;
    let g = -1.268_438 * l3 + 2.609_757_4 * m3 - 0.341_319_4 * s3;
    let bl = -0.004_196_086_3 * l3 - 0.703_418_6 * m3 + 1.707_614_7 * s3;
    Rgb(encode(r), encode(g), encode(bl))
}

/// Linear light → 8-bit sRGB (gamma encode + clamp).
fn encode(x: f32) -> u8 {
    let x = x.clamp(0.0, 1.0);
    let v = if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    };
    (v * 255.0).round() as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_cycle_visits_every_mode_and_returns() {
        let m = ColorMode::default();
        assert_eq!(m, ColorMode::Hands);
        assert_eq!(m.next(), ColorMode::Spectrum);
        assert_eq!(m.next().next(), ColorMode::Accent);
        assert_eq!(m.next().next().next(), ColorMode::Hands);
    }

    #[test]
    fn hands_mode_colours_by_hand() {
        let left = note_color(ColorMode::Hands, 60, Hand::Left, false, false);
        let right = note_color(ColorMode::Hands, 60, Hand::Right, false, false);
        assert_eq!(left, LEFT_HAND);
        assert_eq!(right, RIGHT_HAND);
    }

    #[test]
    fn accent_mode_ignores_hand_and_pitch() {
        for (note, hand) in [(40, Hand::Left), (72, Hand::Right)] {
            assert_eq!(
                note_color(ColorMode::Accent, note, hand, false, false),
                ACCENT
            );
        }
    }

    #[test]
    fn spectrum_colours_by_pitch_class_not_octave() {
        let c4 = note_color(ColorMode::Spectrum, 60, Hand::Right, false, false);
        let c5 = note_color(ColorMode::Spectrum, 72, Hand::Right, false, false);
        let d4 = note_color(ColorMode::Spectrum, 62, Hand::Right, false, false);
        assert_eq!(c4, c5);
        assert_ne!(c4, d4);
    }

    #[test]
    fn spectrum_sets_c_apart_from_b() {
        let c = note_color(ColorMode::Spectrum, 60, Hand::Right, false, false);
        let b = note_color(ColorMode::Spectrum, 59, Hand::Right, false, false);
        let luma = |Rgb(r, g, b): Rgb| 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
        // C is the darker, redder one (red well above blue); B the lighter pink.
        assert!(luma(c) + 40.0 < luma(b), "C {c:?} vs B {b:?}");
        assert!(c.0 as i32 - c.2 as i32 > 100, "C should read red: {c:?}");
        assert!(b.2 as i32 > c.2 as i32, "B should be the bluer pink: {b:?}");
    }

    #[test]
    fn every_mode_separates_active_and_black_keys() {
        for mode in [ColorMode::Hands, ColorMode::Spectrum, ColorMode::Accent] {
            let plain = note_color(mode, 60, Hand::Right, false, false);
            assert_ne!(
                plain,
                note_color(mode, 60, Hand::Right, true, false),
                "{mode:?}"
            );
            assert_ne!(
                plain,
                note_color(mode, 60, Hand::Right, false, true),
                "{mode:?}"
            );
        }
    }

    #[test]
    fn oklch_matches_known_srgb_points() {
        // Pure white and black round-trip exactly.
        assert_eq!(oklch_to_rgb(1.0, 0.0, 0.0), Rgb(255, 255, 255));
        assert_eq!(oklch_to_rgb(0.0, 0.0, 0.0), Rgb(0, 0, 0));
        // sRGB red is oklch(0.628 0.2577 29.23): allow ±2 for rounding.
        let Rgb(r, g, b) = oklch_to_rgb(0.627_955, 0.257_683, 29.234);
        assert!(r >= 253 && g <= 2 && b <= 2, "got {r},{g},{b}");
    }
}
