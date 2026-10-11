//! Fixed-point tempo values.
//!
//! A tempo is stored as an integer count of **milli-BPM** (`120.5` BPM =
//! `120_500`), so it stays `Copy + Eq + Ord + Hash` (an [`Action`] carrying one
//! still derives `Eq`) and all timing math stays integer. On the wire and on
//! disk a tempo is a plain JSON number — an integer when whole (`120`), else a
//! decimal (`120.5`) — and deserialising accepts *any* JSON number, rounding to
//! the nearest milli-BPM. That keeps old `meta.json` files (`"bpm": 120`) and old
//! agent calls (`{"bpm": 120}`) working unchanged.
//!
//! [`Action`]: crate::Action

use serde::de::{self, Visitor};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::fmt;

/// Milli-units per whole unit.
const MILLI: i64 = 1000;

/// A tempo in quarter-note beats per minute, with 0.001 BPM precision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct Bpm(u32);

impl Bpm {
    /// A tempo of `milli` thousandths of a BPM.
    pub const fn from_milli(milli: u32) -> Self {
        Bpm(milli)
    }

    /// A whole-number tempo (`from_whole(120)` = 120 BPM). Saturates rather
    /// than overflowing for absurd inputs.
    pub const fn from_whole(bpm: u32) -> Self {
        Bpm(bpm.saturating_mul(MILLI as u32))
    }

    /// The nearest milli-BPM to `bpm`, or `None` for a non-finite or negative
    /// value. Values beyond the representable range saturate.
    pub fn from_f64(bpm: f64) -> Option<Self> {
        if !bpm.is_finite() || bpm < 0.0 {
            return None;
        }
        // `as` saturates float → int casts.
        Some(Bpm((bpm * MILLI as f64).round() as u32))
    }

    /// Parse user input such as `120`, `120.5` or `97,25` (a comma is accepted
    /// as the decimal separator). Rounds to the nearest milli-BPM; `None` for
    /// anything that isn't a finite, non-negative number.
    pub fn parse(s: &str) -> Option<Self> {
        let s = s.trim().replace(',', ".");
        if s.is_empty() || !s.chars().all(|c| c.is_ascii_digit() || c == '.') {
            return None;
        }
        s.parse::<f64>().ok().and_then(Self::from_f64)
    }

    /// The tempo in milli-BPM.
    pub const fn milli(self) -> u32 {
        self.0
    }

    /// The tempo in BPM as a float (for display / float math only — timing
    /// math should use [`milli`](Bpm::milli)).
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / MILLI as f64
    }

    /// `true` when the tempo is a whole number of BPM.
    pub const fn is_whole(self) -> bool {
        self.0.is_multiple_of(MILLI as u32)
    }

    /// Shift by a signed delta, saturating at `0` and `u32::MAX` milli-BPM.
    pub fn saturating_add(self, delta: BpmDelta) -> Self {
        let next = (self.0 as i64 + delta.milli() as i64).clamp(0, u32::MAX as i64);
        Bpm(next as u32)
    }
}

/// Formats as a plain decimal with trailing zeros trimmed: `120`, `120.5`,
/// `97.25`, `120.001`.
impl fmt::Display for Bpm {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_milli(f, self.0 as i64)
    }
}

/// A signed tempo change with 0.001 BPM precision (`1` BPM = `1000`), used by
/// the tempo-nudge actions. Same flexible number serde as [`Bpm`]: `{"delta":1}`
/// is one BPM, `{"delta":0.1}` a tenth of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Default)]
pub struct BpmDelta(i32);

impl BpmDelta {
    /// A delta of `milli` thousandths of a BPM.
    pub const fn from_milli(milli: i32) -> Self {
        BpmDelta(milli)
    }

    /// A whole-number delta (`from_whole(-5)` = −5 BPM). Saturates.
    pub const fn from_whole(bpm: i32) -> Self {
        BpmDelta(bpm.saturating_mul(MILLI as i32))
    }

    /// The nearest milli-BPM to `delta`, or `None` if it isn't finite.
    /// Out-of-range values saturate.
    pub fn from_f64(delta: f64) -> Option<Self> {
        if !delta.is_finite() {
            return None;
        }
        Some(BpmDelta((delta * MILLI as f64).round() as i32))
    }

    /// The delta in milli-BPM.
    pub const fn milli(self) -> i32 {
        self.0
    }

    /// The delta in BPM as a float.
    pub fn as_f64(self) -> f64 {
        self.0 as f64 / MILLI as f64
    }

    /// The opposite delta (saturating).
    pub const fn neg(self) -> Self {
        BpmDelta(self.0.saturating_neg())
    }
}

impl fmt::Display for BpmDelta {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write_milli(f, self.0 as i64)
    }
}

/// Write a milli-unit count as a trimmed decimal.
fn write_milli(f: &mut fmt::Formatter<'_>, milli: i64) -> fmt::Result {
    let sign = if milli < 0 { "-" } else { "" };
    let abs = milli.unsigned_abs();
    let (whole, frac) = (abs / MILLI as u64, abs % MILLI as u64);
    if frac == 0 {
        return write!(f, "{sign}{whole}");
    }
    let digits = format!("{frac:03}");
    write!(f, "{sign}{whole}.{}", digits.trim_end_matches('0'))
}

/// Serialise a milli count as an integer when whole, else a decimal.
fn serialize_milli<S: Serializer>(milli: i64, s: S) -> Result<S::Ok, S::Error> {
    if milli % MILLI == 0 {
        s.serialize_i64(milli / MILLI)
    } else {
        s.serialize_f64(milli as f64 / MILLI as f64)
    }
}

impl Serialize for Bpm {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_milli(self.0 as i64, s)
    }
}

impl Serialize for BpmDelta {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_milli(self.0 as i64, s)
    }
}

/// Accepts any JSON number and yields it in BPM as an `f64`, for rounding to
/// milli. Integers are converted exactly before scaling where possible.
struct NumberVisitor {
    signed: bool,
}

impl Visitor<'_> for NumberVisitor {
    type Value = f64;

    fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.signed {
            f.write_str("a finite number of BPM (0.001 precision)")
        } else {
            f.write_str("a finite, non-negative number of BPM (0.001 precision)")
        }
    }

    fn visit_u64<E: de::Error>(self, v: u64) -> Result<f64, E> {
        Ok(v as f64)
    }

    fn visit_i64<E: de::Error>(self, v: i64) -> Result<f64, E> {
        if !self.signed && v < 0 {
            return Err(E::invalid_value(de::Unexpected::Signed(v), &self));
        }
        Ok(v as f64)
    }

    fn visit_f64<E: de::Error>(self, v: f64) -> Result<f64, E> {
        if !v.is_finite() || (!self.signed && v < 0.0) {
            return Err(E::invalid_value(de::Unexpected::Float(v), &self));
        }
        Ok(v)
    }
}

impl<'de> Deserialize<'de> for Bpm {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = d.deserialize_any(NumberVisitor { signed: false })?;
        Bpm::from_f64(v).ok_or_else(|| de::Error::custom("invalid BPM"))
    }
}

impl<'de> Deserialize<'de> for BpmDelta {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = d.deserialize_any(NumberVisitor { signed: true })?;
        BpmDelta::from_f64(v).ok_or_else(|| de::Error::custom("invalid BPM delta"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn constructors_and_accessors() {
        assert_eq!(Bpm::from_whole(120).milli(), 120_000);
        assert_eq!(Bpm::from_milli(120_500).as_f64(), 120.5);
        assert!(Bpm::from_whole(97).is_whole());
        assert!(!Bpm::from_milli(97_250).is_whole());
        assert_eq!(Bpm::from_f64(97.25), Some(Bpm::from_milli(97_250)));
        assert_eq!(Bpm::from_f64(120.0004), Some(Bpm::from_whole(120)));
        assert_eq!(Bpm::from_f64(-1.0), None);
        assert_eq!(Bpm::from_f64(f64::NAN), None);
        assert_eq!(BpmDelta::from_f64(-0.1), Some(BpmDelta::from_milli(-100)));
        assert_eq!(BpmDelta::from_whole(-5).neg(), BpmDelta::from_whole(5));
    }

    #[test]
    fn display_trims_trailing_zeros() {
        assert_eq!(Bpm::from_whole(120).to_string(), "120");
        assert_eq!(Bpm::from_milli(120_500).to_string(), "120.5");
        assert_eq!(Bpm::from_milli(97_250).to_string(), "97.25");
        assert_eq!(Bpm::from_milli(120_001).to_string(), "120.001");
        assert_eq!(BpmDelta::from_milli(-100).to_string(), "-0.1");
        assert_eq!(BpmDelta::from_whole(-5).to_string(), "-5");
    }

    #[test]
    fn parse_accepts_dot_and_comma() {
        assert_eq!(Bpm::parse("120"), Some(Bpm::from_whole(120)));
        assert_eq!(Bpm::parse(" 120.5 "), Some(Bpm::from_milli(120_500)));
        assert_eq!(Bpm::parse("97,25"), Some(Bpm::from_milli(97_250)));
        assert_eq!(Bpm::parse(".5"), Some(Bpm::from_milli(500)));
        assert_eq!(Bpm::parse(""), None);
        assert_eq!(Bpm::parse("-3"), None);
        assert_eq!(Bpm::parse("1e3"), None);
        assert_eq!(Bpm::parse("abc"), None);
        assert_eq!(Bpm::parse("1.2.3"), None);
    }

    #[test]
    fn serialises_as_int_when_whole_else_decimal() {
        assert_eq!(
            serde_json::to_value(Bpm::from_whole(120)).unwrap(),
            json!(120)
        );
        assert_eq!(serde_json::to_string(&Bpm::from_whole(120)).unwrap(), "120");
        assert_eq!(
            serde_json::to_string(&Bpm::from_milli(120_500)).unwrap(),
            "120.5"
        );
        assert_eq!(
            serde_json::to_string(&Bpm::from_milli(97_250)).unwrap(),
            "97.25"
        );
        assert_eq!(
            serde_json::to_string(&BpmDelta::from_milli(-100)).unwrap(),
            "-0.1"
        );
        assert_eq!(
            serde_json::to_string(&BpmDelta::from_whole(-5)).unwrap(),
            "-5"
        );
    }

    #[test]
    fn deserialises_any_number_rounding_to_milli() {
        let b = |v| serde_json::from_value::<Bpm>(v);
        assert_eq!(b(json!(120)).unwrap(), Bpm::from_whole(120));
        assert_eq!(b(json!(120.0)).unwrap(), Bpm::from_whole(120));
        assert_eq!(b(json!(120.5)).unwrap(), Bpm::from_milli(120_500));
        assert_eq!(b(json!(97.2504)).unwrap(), Bpm::from_milli(97_250));
        assert!(b(json!(-1)).is_err());
        assert!(b(json!(-0.5)).is_err());
        assert!(b(json!("120")).is_err());

        let d = |v| serde_json::from_value::<BpmDelta>(v);
        assert_eq!(d(json!(1)).unwrap(), BpmDelta::from_whole(1));
        assert_eq!(d(json!(-10)).unwrap(), BpmDelta::from_whole(-10));
        assert_eq!(d(json!(0.1)).unwrap(), BpmDelta::from_milli(100));
        assert_eq!(d(json!(-0.25)).unwrap(), BpmDelta::from_milli(-250));
    }

    #[test]
    fn round_trips_through_json() {
        for milli in [20_000u32, 120_000, 120_500, 97_250, 120_001, 299_999] {
            let bpm = Bpm::from_milli(milli);
            let s = serde_json::to_string(&bpm).unwrap();
            assert_eq!(serde_json::from_str::<Bpm>(&s).unwrap(), bpm, "{s}");
        }
    }
}
