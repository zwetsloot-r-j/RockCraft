//! Custom background images with keyframed transforms (M14-D).
//!
//! A piece can carry any number of **background image layers**. Each layer is a
//! bundle-relative file plus a list of [`Keyframe`]s on the song timeline; the
//! layer's [`Transform`] at a given song time is *interpolated* between the
//! surrounding keyframes, so a still image pans, zooms, rotates and fades as the
//! song plays.
//!
//! This module is the whole model **and** the whole interpolation math — pure,
//! headless-testable, no image decoding and no rendering. Frontends receive an
//! already-evaluated [`Transform`] (via [`BackgroundView`]) and apply it
//! verbatim; nothing re-implements the curve.
//!
//! It sits beside — never replaces — the single [`crate::BackgroundVideo`]
//! backdrop (M9-G): a piece can have the movie *and* images.
//!
//! ## Coordinate system
//!
//! [`Transform`] is expressed in **normalised surface units** so it is
//! resolution-independent and a webview can map it straight onto a CSS
//! transform of an `object-fit: contain` image:
//!
//! ```text
//! translate(x · surface_width, y · surface_height) rotate(rotation_deg) scale(scale)
//! ```
//!
//! [`Transform::IDENTITY`] is therefore the untouched image: centred, contained,
//! unrotated, fully opaque.

use serde::{Deserialize, Serialize};

/// Largest absolute pan offset, in surface widths/heights. Four screens out is
/// already fully off-stage; the clamp only exists to keep a fat-fingered nudge
/// (or a hand-edited `meta.json`) from parking a layer at 10^9.
pub const POS_LIMIT: f32 = 4.0;

/// Smallest allowed scale. Below this a layer is a dot, not a background.
pub const MIN_SCALE: f32 = 0.05;

/// Largest allowed scale.
pub const MAX_SCALE: f32 = 10.0;

/// Coerce a possibly non-finite float to `fallback`.
///
/// `NaN`/`±inf` can only reach us from a hand-edited bundle or a frontend bug;
/// letting either into a transform would poison every later `lerp`, so it is
/// replaced at the boundary rather than propagated.
fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() {
        value
    } else {
        fallback
    }
}

/// Wrap an angle in degrees into `(-180, 180]`.
fn wrap_deg(deg: f32) -> f32 {
    if !deg.is_finite() {
        return 0.0;
    }
    let wrapped = deg % 360.0;
    if wrapped > 180.0 {
        wrapped - 360.0
    } else if wrapped <= -180.0 {
        wrapped + 360.0
    } else {
        wrapped
    }
}

/// Where a background image sits on the play surface, and how visible it is.
///
/// See the module docs for the coordinate system. Every constructor clamps, so
/// a `Transform` in hand is always in range and always finite.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    /// Horizontal offset of the image centre from the surface centre, in
    /// surface widths. `0.0` is centred; clamped to `±`[`POS_LIMIT`].
    pub x: f32,
    /// Vertical offset of the image centre from the surface centre, in surface
    /// heights. `0.0` is centred; clamped to `±`[`POS_LIMIT`].
    pub y: f32,
    /// Multiplier on the contained fit. `1.0` is "as large as fits"; clamped to
    /// [`MIN_SCALE`]`..=`[`MAX_SCALE`].
    pub scale: f32,
    /// Clockwise rotation about the image centre, wrapped into `(-180, 180]`.
    pub rotation_deg: f32,
    /// Layer opacity, clamped to `0.0..=1.0`.
    pub opacity: f32,
}

impl Transform {
    /// The untouched image: centred, contained, unrotated, fully opaque.
    pub const IDENTITY: Transform = Transform {
        x: 0.0,
        y: 0.0,
        scale: 1.0,
        rotation_deg: 0.0,
        opacity: 1.0,
    };

    /// Build a transform, clamping every component into range.
    pub fn new(x: f32, y: f32, scale: f32, rotation_deg: f32, opacity: f32) -> Self {
        Transform {
            x,
            y,
            scale,
            rotation_deg,
            opacity,
        }
        .clamped()
    }

    /// Return this transform with every component clamped/wrapped into range and
    /// any non-finite component replaced by the identity's.
    pub fn clamped(self) -> Self {
        Transform {
            x: finite_or(self.x, 0.0).clamp(-POS_LIMIT, POS_LIMIT),
            y: finite_or(self.y, 0.0).clamp(-POS_LIMIT, POS_LIMIT),
            scale: finite_or(self.scale, 1.0).clamp(MIN_SCALE, MAX_SCALE),
            rotation_deg: wrap_deg(self.rotation_deg),
            opacity: finite_or(self.opacity, 1.0).clamp(0.0, 1.0),
        }
    }

    /// Linear blend from `a` to `b` at `t`, with `t` clamped to `0.0..=1.0`.
    ///
    /// Rotation takes the **shortest arc**, so 170° → -170° travels 20° rather
    /// than sweeping 340° the other way round.
    pub fn lerp(a: Transform, b: Transform, t: f32) -> Transform {
        let t = finite_or(t, 0.0).clamp(0.0, 1.0);
        let mix = |from: f32, to: f32| from + (to - from) * t;
        // Shortest-arc rotation: walk the wrapped delta, then re-wrap.
        let delta = wrap_deg(b.rotation_deg - a.rotation_deg);
        Transform {
            x: mix(a.x, b.x),
            y: mix(a.y, b.y),
            scale: mix(a.scale, b.scale),
            rotation_deg: a.rotation_deg + delta * t,
            opacity: mix(a.opacity, b.opacity),
        }
        .clamped()
    }
}

impl Default for Transform {
    fn default() -> Self {
        Transform::IDENTITY
    }
}

/// The curve a keyframe uses on the segment **leaving** it.
///
/// The easing of the *earlier* keyframe shapes the segment between the two, so a
/// keyframe's easing is "how the animation departs from here".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    /// Constant velocity.
    #[default]
    Linear,
    /// Accelerate away from this keyframe (quadratic).
    EaseIn,
    /// Decelerate into the next keyframe (quadratic).
    EaseOut,
    /// Accelerate, then decelerate (quadratic in/out).
    EaseInOut,
    /// A cut: hold this keyframe's transform until the next one is reached.
    Hold,
}

impl Easing {
    /// Shape a segment parameter `t` (clamped to `0.0..=1.0`).
    ///
    /// Every curve maps `0 → 0` and `1 → 1`, so the keyframes themselves are hit
    /// exactly whichever easing is chosen.
    pub fn apply(self, t: f32) -> f32 {
        let t = finite_or(t, 0.0).clamp(0.0, 1.0);
        match self {
            Easing::Linear => t,
            Easing::EaseIn => t * t,
            Easing::EaseOut => {
                let inv = 1.0 - t;
                1.0 - inv * inv
            }
            Easing::EaseInOut => {
                if t < 0.5 {
                    2.0 * t * t
                } else {
                    let inv = -2.0 * t + 2.0;
                    1.0 - (inv * inv) / 2.0
                }
            }
            Easing::Hold => {
                if t >= 1.0 {
                    1.0
                } else {
                    0.0
                }
            }
        }
    }

    /// Stable snake_case name, identical to the serde tag — for status lines.
    pub fn name(self) -> &'static str {
        match self {
            Easing::Linear => "linear",
            Easing::EaseIn => "ease_in",
            Easing::EaseOut => "ease_out",
            Easing::EaseInOut => "ease_in_out",
            Easing::Hold => "hold",
        }
    }
}

/// One pinned `(song time, transform)` pair on a background layer.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Keyframe {
    /// Song time this transform is reached, in microseconds.
    pub time_us: u64,
    /// The layout at `time_us`.
    pub transform: Transform,
    /// Curve applied on the segment leaving this keyframe.
    #[serde(default)]
    pub easing: Easing,
}

impl Keyframe {
    /// A keyframe at `time_us`, clamping the transform into range.
    pub fn new(time_us: u64, transform: Transform, easing: Easing) -> Self {
        Keyframe {
            time_us,
            transform: transform.clamped(),
            easing,
        }
    }
}

/// One background image layer: a bundle-relative file plus its animation.
///
/// `file` is the bundle-relative filename only (e.g. `"background-1.png"`) —
/// never an absolute path, so the bundle stays movable, exactly like
/// [`crate::BackgroundVideo`]. `core` never decodes the image; frontends resolve
/// and draw it.
///
/// `keyframes` is kept sorted by `time_us` with at most one keyframe per time.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundImage {
    /// Stable id used to address this layer over the control protocol.
    pub id: String,
    /// Bundle-relative filename.
    pub file: String,
    /// Sorted, de-duplicated keyframes. Empty means "static at
    /// [`Transform::IDENTITY`]".
    #[serde(default)]
    pub keyframes: Vec<Keyframe>,
}

impl BackgroundImage {
    /// A layer with no keyframes — a still, centred, fully opaque backdrop.
    pub fn new(id: impl Into<String>, file: impl Into<String>) -> Self {
        BackgroundImage {
            id: id.into(),
            file: file.into(),
            keyframes: Vec::new(),
        }
    }

    /// The layout at song time `us`.
    ///
    /// No keyframes → [`Transform::IDENTITY`]. Before the first / after the last
    /// keyframe the nearest one holds (no extrapolation). Between two, the
    /// earlier keyframe's [`Easing`] shapes a [`Transform::lerp`].
    pub fn transform_at(&self, us: u64) -> Transform {
        let frames = &self.keyframes;
        let Some(first) = frames.first() else {
            return Transform::IDENTITY;
        };
        if us <= first.time_us {
            return first.transform;
        }
        let last = frames.last().expect("non-empty: `first` matched");
        if us >= last.time_us {
            return last.transform;
        }
        // `us` is strictly inside the span, so a following keyframe exists.
        let next = frames
            .iter()
            .position(|k| k.time_us > us)
            .expect("us < last.time_us");
        let a = frames[next - 1];
        let b = frames[next];
        let span = b.time_us - a.time_us;
        if span == 0 {
            return b.transform;
        }
        let raw = (us - a.time_us) as f32 / span as f32;
        Transform::lerp(a.transform, b.transform, a.easing.apply(raw))
    }

    /// Index of the keyframe sitting at exactly `time_us`, if any.
    pub fn keyframe_index_at(&self, time_us: u64) -> Option<usize> {
        self.keyframes.iter().position(|k| k.time_us == time_us)
    }

    /// The keyframe sitting at exactly `time_us`, if any.
    pub fn keyframe_at(&self, time_us: u64) -> Option<&Keyframe> {
        self.keyframe_index_at(time_us).map(|i| &self.keyframes[i])
    }

    /// Pin `transform` at `time_us`, replacing any keyframe already there.
    ///
    /// Returns the index the keyframe now occupies; the list stays sorted.
    pub fn set_keyframe(&mut self, time_us: u64, transform: Transform, easing: Easing) -> usize {
        let frame = Keyframe::new(time_us, transform, easing);
        match self.keyframe_index_at(time_us) {
            Some(i) => {
                self.keyframes[i] = frame;
                i
            }
            None => {
                let at = self
                    .keyframes
                    .iter()
                    .position(|k| k.time_us > time_us)
                    .unwrap_or(self.keyframes.len());
                self.keyframes.insert(at, frame);
                at
            }
        }
    }

    /// Drop the keyframe at exactly `time_us`. Returns whether one was removed.
    pub fn remove_keyframe_at(&mut self, time_us: u64) -> bool {
        match self.keyframe_index_at(time_us) {
            Some(i) => {
                self.keyframes.remove(i);
                true
            }
            None => false,
        }
    }

    /// Set the departing curve of the keyframe at exactly `time_us`. Returns
    /// whether a keyframe was there to change.
    pub fn set_easing_at(&mut self, time_us: u64, easing: Easing) -> bool {
        match self.keyframe_index_at(time_us) {
            Some(i) => {
                self.keyframes[i].easing = easing;
                true
            }
            None => false,
        }
    }

    /// Shift every keyframe in time by `delta_us`, saturating at 0.
    ///
    /// Used when a piece is sliced into segments: the animation must travel with
    /// the notes and media offsets it was authored against.
    pub fn shift_keyframes(&mut self, delta_us: i64) {
        for k in &mut self.keyframes {
            k.time_us = k.time_us.saturating_add_signed(delta_us);
        }
        // A saturating shift can collide two keyframes at 0; normalise re-sorts
        // and keeps the last one written at each time.
        self.normalize();
    }

    /// The layer as it appears inside the song-time slice `[start_us, end_us)`,
    /// with times rebased so `start_us` becomes 0.
    ///
    /// The animation is **resampled**, not merely shifted: the transform the
    /// layer had at each edge is pinned as a keyframe, so a split part opens and
    /// closes on exactly the layout the whole piece had there. Keyframes outside
    /// the slice are dropped rather than saturated onto the edges.
    pub fn sliced(&self, start_us: u64, end_us: u64) -> BackgroundImage {
        let mut out = BackgroundImage {
            id: self.id.clone(),
            file: self.file.clone(),
            keyframes: Vec::new(),
        };
        if self.keyframes.is_empty() || end_us <= start_us {
            return out;
        }
        // The curve in force at the slice's opening edge is the one departing
        // the last keyframe at or before it.
        let opening_easing = self
            .keyframes
            .iter()
            .rev()
            .find(|k| k.time_us <= start_us)
            .map(|k| k.easing)
            .unwrap_or_default();
        out.keyframes.push(Keyframe::new(
            0,
            self.transform_at(start_us),
            opening_easing,
        ));
        for k in &self.keyframes {
            if k.time_us > start_us && k.time_us < end_us {
                out.keyframes
                    .push(Keyframe::new(k.time_us - start_us, k.transform, k.easing));
            }
        }
        out.keyframes.push(Keyframe::new(
            end_us - start_us,
            self.transform_at(end_us),
            Easing::Linear,
        ));
        out.normalize();
        out
    }

    /// Restore the sorted / de-duplicated / clamped invariants.
    ///
    /// Called after deserialising a hand-editable `meta.json`, where the
    /// keyframe list is just an array and none of the invariants are enforced by
    /// the format. When two keyframes collide on a time, the earlier-listed one
    /// wins.
    pub fn normalize(&mut self) {
        self.keyframes.sort_by_key(|k| k.time_us);
        self.keyframes.dedup_by_key(|k| k.time_us);
        for k in &mut self.keyframes {
            k.transform = k.transform.clamped();
        }
    }
}

/// The piece's background layers plus the one edit actions address.
///
/// Layers render back-to-front in list order (index 0 furthest back). The
/// selected index is editor state, not part of the piece — it is clamped into
/// range on every mutation so `selected()` is `Some` exactly when the stack is
/// non-empty.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct BackgroundStack {
    layers: Vec<BackgroundImage>,
    #[serde(default)]
    selected: usize,
}

impl BackgroundStack {
    /// An empty stack.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a stack from persisted layers, normalising each and selecting the
    /// first.
    pub fn from_layers(layers: Vec<BackgroundImage>) -> Self {
        let mut stack = BackgroundStack {
            layers,
            selected: 0,
        };
        for layer in &mut stack.layers {
            layer.normalize();
        }
        stack
    }

    /// The layers, back-to-front.
    pub fn layers(&self) -> &[BackgroundImage] {
        &self.layers
    }

    /// Consume the stack, yielding its layers (for persistence).
    pub fn into_layers(self) -> Vec<BackgroundImage> {
        self.layers
    }

    /// Number of layers.
    pub fn len(&self) -> usize {
        self.layers.len()
    }

    /// Whether the piece has no background images.
    pub fn is_empty(&self) -> bool {
        self.layers.is_empty()
    }

    /// Index of the layer edit actions address; `None` when empty.
    pub fn selected_index(&self) -> Option<usize> {
        if self.layers.is_empty() {
            None
        } else {
            Some(self.selected.min(self.layers.len() - 1))
        }
    }

    /// The layer edit actions address; `None` when empty.
    pub fn selected(&self) -> Option<&BackgroundImage> {
        self.selected_index().map(|i| &self.layers[i])
    }

    /// Mutable access to the addressed layer; `None` when empty.
    pub fn selected_mut(&mut self) -> Option<&mut BackgroundImage> {
        self.selected_index().map(|i| &mut self.layers[i])
    }

    /// Address layer `index`. Returns whether the index existed.
    pub fn select(&mut self, index: usize) -> bool {
        if index < self.layers.len() {
            self.selected = index;
            true
        } else {
            false
        }
    }

    /// Move the selection by `delta`, wrapping in both directions. No-op when
    /// the stack is empty.
    pub fn cycle(&mut self, delta: i32) {
        let len = self.layers.len();
        if len == 0 {
            return;
        }
        let len_i = len as i64;
        let current = self.selected.min(len - 1) as i64;
        let next = (current + delta as i64).rem_euclid(len_i);
        self.selected = next as usize;
    }

    /// Append a layer and address it. Returns its index.
    pub fn push(&mut self, mut layer: BackgroundImage) -> usize {
        layer.normalize();
        self.layers.push(layer);
        self.selected = self.layers.len() - 1;
        self.selected
    }

    /// Remove layer `index`, keeping the selection in range.
    pub fn remove(&mut self, index: usize) -> Option<BackgroundImage> {
        if index >= self.layers.len() {
            return None;
        }
        let removed = self.layers.remove(index);
        if self.selected >= self.layers.len() {
            self.selected = self.layers.len().saturating_sub(1);
        }
        Some(removed)
    }

    /// Remove the layer with this id, keeping the selection in range.
    pub fn remove_by_id(&mut self, id: &str) -> Option<BackgroundImage> {
        let index = self.layers.iter().position(|l| l.id == id)?;
        self.remove(index)
    }

    /// Whether a layer with this id exists.
    pub fn contains_id(&self, id: &str) -> bool {
        self.layers.iter().any(|l| l.id == id)
    }

    /// Mutable access to a layer by id.
    pub fn get_by_id_mut(&mut self, id: &str) -> Option<&mut BackgroundImage> {
        self.layers.iter_mut().find(|l| l.id == id)
    }

    /// Every layer with its transform evaluated at song time `us` — the shape a
    /// frontend renders straight from.
    pub fn views_at(&self, us: u64) -> Vec<BackgroundView> {
        let selected = self.selected_index();
        self.layers
            .iter()
            .enumerate()
            .map(|(index, layer)| BackgroundView {
                index,
                id: layer.id.clone(),
                file: layer.file.clone(),
                selected: selected == Some(index),
                transform: layer.transform_at(us),
                keyframes: layer.keyframes.clone(),
            })
            .collect()
    }

    /// Shift every layer's keyframes in time (segment slicing).
    pub fn shift_keyframes(&mut self, delta_us: i64) {
        for layer in &mut self.layers {
            layer.shift_keyframes(delta_us);
        }
    }
}

/// One background layer as a frontend sees it: the file to draw plus the
/// transform **already evaluated** at the current time.
///
/// `keyframes` rides along so an editor can draw the keyframe ruler; a renderer
/// that only paints the layer ignores it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BackgroundView {
    /// Position in the stack, back-to-front.
    pub index: usize,
    /// Stable layer id.
    pub id: String,
    /// Bundle-relative filename.
    pub file: String,
    /// Whether edit actions currently address this layer.
    pub selected: bool,
    /// Layout at the evaluated time.
    pub transform: Transform,
    /// The layer's keyframes, for a timeline ruler.
    pub keyframes: Vec<Keyframe>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Float comparison tolerant of the interpolation's rounding.
    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-5
    }

    // ── Transform ───────────────────────────────────────────────────────────

    #[test]
    fn identity_is_centred_contained_opaque() {
        let t = Transform::IDENTITY;
        assert_eq!(
            (t.x, t.y, t.scale, t.rotation_deg, t.opacity),
            (0.0, 0.0, 1.0, 0.0, 1.0)
        );
        assert_eq!(Transform::default(), Transform::IDENTITY);
    }

    #[test]
    fn new_clamps_every_component() {
        let t = Transform::new(99.0, -99.0, 500.0, 0.0, 3.0);
        assert_eq!(t.x, POS_LIMIT);
        assert_eq!(t.y, -POS_LIMIT);
        assert_eq!(t.scale, MAX_SCALE);
        assert_eq!(t.opacity, 1.0);

        let t = Transform::new(0.0, 0.0, 0.0, 0.0, -1.0);
        assert_eq!(t.scale, MIN_SCALE);
        assert_eq!(t.opacity, 0.0);
    }

    #[test]
    fn rotation_wraps_into_half_open_turn() {
        assert!(close(
            Transform::new(0.0, 0.0, 1.0, 190.0, 1.0).rotation_deg,
            -170.0
        ));
        assert!(close(
            Transform::new(0.0, 0.0, 1.0, -190.0, 1.0).rotation_deg,
            170.0
        ));
        assert!(close(
            Transform::new(0.0, 0.0, 1.0, 540.0, 1.0).rotation_deg,
            180.0
        ));
        assert!(close(
            Transform::new(0.0, 0.0, 1.0, 0.0, 1.0).rotation_deg,
            0.0
        ));
    }

    #[test]
    fn non_finite_components_fall_back_to_identity() {
        let t = Transform::new(
            f32::NAN,
            f32::INFINITY,
            f32::NAN,
            f32::NAN,
            f32::NEG_INFINITY,
        );
        assert_eq!(t, Transform::IDENTITY);
    }

    #[test]
    fn lerp_hits_both_ends_and_the_midpoint() {
        let a = Transform::new(-1.0, 0.0, 1.0, 0.0, 1.0);
        let b = Transform::new(1.0, 0.5, 2.0, 90.0, 0.0);
        assert_eq!(Transform::lerp(a, b, 0.0), a);
        assert_eq!(Transform::lerp(a, b, 1.0), b);
        let mid = Transform::lerp(a, b, 0.5);
        assert!(close(mid.x, 0.0));
        assert!(close(mid.y, 0.25));
        assert!(close(mid.scale, 1.5));
        assert!(close(mid.rotation_deg, 45.0));
        assert!(close(mid.opacity, 0.5));
    }

    #[test]
    fn lerp_clamps_t_outside_the_unit_range() {
        let a = Transform::new(-1.0, 0.0, 1.0, 0.0, 1.0);
        let b = Transform::new(1.0, 0.0, 1.0, 0.0, 1.0);
        assert_eq!(Transform::lerp(a, b, -5.0), a);
        assert_eq!(Transform::lerp(a, b, 5.0), b);
        assert_eq!(Transform::lerp(a, b, f32::NAN), a);
    }

    #[test]
    fn lerp_rotation_takes_the_shortest_arc() {
        // 170° → -170° is 20° forwards across the seam, not 340° backwards.
        let a = Transform::new(0.0, 0.0, 1.0, 170.0, 1.0);
        let b = Transform::new(0.0, 0.0, 1.0, -170.0, 1.0);
        let mid = Transform::lerp(a, b, 0.5);
        assert!(close(mid.rotation_deg, 180.0), "got {}", mid.rotation_deg);
        // And back the other way.
        let mid = Transform::lerp(b, a, 0.5);
        assert!(close(mid.rotation_deg, 180.0), "got {}", mid.rotation_deg);
    }

    // ── Easing ──────────────────────────────────────────────────────────────

    #[test]
    fn every_easing_pins_both_ends() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::Hold,
        ] {
            assert!(close(e.apply(0.0), 0.0), "{e:?} at 0");
            assert!(close(e.apply(1.0), 1.0), "{e:?} at 1");
            assert!(!e.name().is_empty());
        }
    }

    #[test]
    fn easing_curves_differ_at_the_midpoint() {
        assert!(close(Easing::Linear.apply(0.5), 0.5));
        assert!(close(Easing::EaseIn.apply(0.5), 0.25));
        assert!(close(Easing::EaseOut.apply(0.5), 0.75));
        assert!(close(Easing::EaseInOut.apply(0.5), 0.5));
        // …but ease-in-out is *not* linear away from the midpoint.
        assert!(Easing::EaseInOut.apply(0.25) < 0.25);
        assert!(Easing::EaseInOut.apply(0.75) > 0.75);
    }

    #[test]
    fn hold_is_a_cut() {
        assert!(close(Easing::Hold.apply(0.0), 0.0));
        assert!(close(Easing::Hold.apply(0.99), 0.0));
        assert!(close(Easing::Hold.apply(1.0), 1.0));
    }

    #[test]
    fn easing_default_is_linear() {
        assert_eq!(Easing::default(), Easing::Linear);
    }

    // ── BackgroundImage::transform_at ───────────────────────────────────────

    fn layer_with(frames: &[(u64, f32, Easing)]) -> BackgroundImage {
        let mut layer = BackgroundImage::new("bg1", "bg.png");
        for &(time_us, x, easing) in frames {
            layer.set_keyframe(time_us, Transform::new(x, 0.0, 1.0, 0.0, 1.0), easing);
        }
        layer
    }

    #[test]
    fn no_keyframes_is_static_identity() {
        let layer = BackgroundImage::new("bg1", "bg.png");
        assert_eq!(layer.transform_at(0), Transform::IDENTITY);
        assert_eq!(layer.transform_at(9_999_999), Transform::IDENTITY);
    }

    #[test]
    fn one_keyframe_is_constant_everywhere() {
        let layer = layer_with(&[(1_000_000, 0.5, Easing::Linear)]);
        assert!(close(layer.transform_at(0).x, 0.5));
        assert!(close(layer.transform_at(1_000_000).x, 0.5));
        assert!(close(layer.transform_at(9_000_000).x, 0.5));
    }

    #[test]
    fn outside_the_span_the_nearest_keyframe_holds() {
        let layer = layer_with(&[
            (1_000_000, 0.0, Easing::Linear),
            (2_000_000, 1.0, Easing::Linear),
        ]);
        // Before the first: no extrapolation backwards.
        assert!(close(layer.transform_at(0).x, 0.0));
        assert!(close(layer.transform_at(999_999).x, 0.0));
        // After the last: no extrapolation forwards.
        assert!(close(layer.transform_at(5_000_000).x, 1.0));
    }

    #[test]
    fn between_keyframes_interpolates_linearly() {
        let layer = layer_with(&[
            (1_000_000, 0.0, Easing::Linear),
            (2_000_000, 1.0, Easing::Linear),
        ]);
        assert!(close(layer.transform_at(1_500_000).x, 0.5));
        assert!(close(layer.transform_at(1_250_000).x, 0.25));
    }

    #[test]
    fn the_earlier_keyframes_easing_shapes_the_segment() {
        let eased = layer_with(&[(0, 0.0, Easing::EaseIn), (1_000_000, 1.0, Easing::Linear)]);
        // ease-in at the halfway point is 0.25, not 0.5.
        assert!(close(eased.transform_at(500_000).x, 0.25));
        // A `Hold` segment does not move until the next keyframe is reached.
        let held = layer_with(&[(0, 0.0, Easing::Hold), (1_000_000, 1.0, Easing::Linear)]);
        assert!(close(held.transform_at(999_999).x, 0.0));
        assert!(close(held.transform_at(1_000_000).x, 1.0));
    }

    #[test]
    fn interpolation_spans_the_right_pair_with_three_keyframes() {
        let layer = layer_with(&[
            (0, 0.0, Easing::Linear),
            (1_000_000, 1.0, Easing::Linear),
            (3_000_000, -1.0, Easing::Linear),
        ]);
        assert!(close(layer.transform_at(500_000).x, 0.5));
        assert!(close(layer.transform_at(2_000_000).x, 0.0));
    }

    // ── keyframe list maintenance ───────────────────────────────────────────

    #[test]
    fn set_keyframe_inserts_sorted_and_replaces_in_place() {
        let mut layer = BackgroundImage::new("bg1", "bg.png");
        layer.set_keyframe(2_000_000, Transform::IDENTITY, Easing::Linear);
        layer.set_keyframe(1_000_000, Transform::IDENTITY, Easing::Linear);
        layer.set_keyframe(3_000_000, Transform::IDENTITY, Easing::Linear);
        let times: Vec<u64> = layer.keyframes.iter().map(|k| k.time_us).collect();
        assert_eq!(times, vec![1_000_000, 2_000_000, 3_000_000]);

        // Same time again replaces rather than duplicating.
        let at = layer.set_keyframe(
            2_000_000,
            Transform::new(0.25, 0.0, 1.0, 0.0, 1.0),
            Easing::Hold,
        );
        assert_eq!(at, 1);
        assert_eq!(layer.keyframes.len(), 3);
        assert!(close(layer.keyframes[1].transform.x, 0.25));
        assert_eq!(layer.keyframes[1].easing, Easing::Hold);
    }

    #[test]
    fn remove_and_easing_target_an_exact_time() {
        let mut layer = layer_with(&[
            (1_000_000, 0.0, Easing::Linear),
            (2_000_000, 1.0, Easing::Linear),
        ]);
        assert!(!layer.remove_keyframe_at(1_500_000));
        assert!(!layer.set_easing_at(1_500_000, Easing::Hold));
        assert!(layer.set_easing_at(1_000_000, Easing::Hold));
        assert_eq!(layer.keyframe_at(1_000_000).unwrap().easing, Easing::Hold);
        assert!(layer.remove_keyframe_at(1_000_000));
        assert_eq!(layer.keyframes.len(), 1);
        assert!(layer.keyframe_at(1_000_000).is_none());
    }

    #[test]
    fn normalize_sorts_dedupes_and_clamps() {
        let mut layer = BackgroundImage::new("bg1", "bg.png");
        // Hand-built (as a bad meta.json would be): unsorted, duplicated, wild.
        layer.keyframes = vec![
            Keyframe {
                time_us: 2_000_000,
                transform: Transform {
                    x: 99.0,
                    y: 0.0,
                    scale: 1.0,
                    rotation_deg: 0.0,
                    opacity: 1.0,
                },
                easing: Easing::Linear,
            },
            Keyframe {
                time_us: 1_000_000,
                transform: Transform::IDENTITY,
                easing: Easing::Linear,
            },
            Keyframe {
                time_us: 2_000_000,
                transform: Transform::IDENTITY,
                easing: Easing::Hold,
            },
        ];
        layer.normalize();
        let times: Vec<u64> = layer.keyframes.iter().map(|k| k.time_us).collect();
        assert_eq!(times, vec![1_000_000, 2_000_000]);
        assert!(layer
            .keyframes
            .iter()
            .all(|k| k.transform.x.abs() <= POS_LIMIT));
    }

    #[test]
    fn shift_keyframes_moves_the_animation_and_saturates_at_zero() {
        let mut layer = layer_with(&[
            (1_000_000, 0.0, Easing::Linear),
            (2_000_000, 1.0, Easing::Linear),
        ]);
        layer.shift_keyframes(500_000);
        let times: Vec<u64> = layer.keyframes.iter().map(|k| k.time_us).collect();
        assert_eq!(times, vec![1_500_000, 2_500_000]);

        // Shifting past zero clamps; colliding keyframes collapse to one.
        layer.shift_keyframes(-3_000_000);
        let times: Vec<u64> = layer.keyframes.iter().map(|k| k.time_us).collect();
        assert_eq!(times, vec![0]);
    }

    #[test]
    fn sliced_resamples_the_animation_onto_the_new_zero() {
        // A 4 s pan from 0.0 to 1.0; slice the middle two seconds.
        let layer = layer_with(&[(0, 0.0, Easing::Linear), (4_000_000, 1.0, Easing::Linear)]);
        let cut = layer.sliced(1_000_000, 3_000_000);
        // Both edges are pinned at the transform the whole piece had there …
        assert!(close(cut.transform_at(0).x, 0.25));
        assert!(close(cut.transform_at(2_000_000).x, 0.75));
        // … and the motion in between still matches the original.
        assert!(close(cut.transform_at(1_000_000).x, 0.5));
        assert_eq!(cut.id, layer.id);
        assert_eq!(cut.file, layer.file);
    }

    #[test]
    fn sliced_keeps_interior_keyframes_and_drops_outside_ones() {
        let layer = layer_with(&[
            (0, 0.0, Easing::Linear),
            (1_000_000, 1.0, Easing::Hold),
            (2_000_000, -1.0, Easing::Linear),
            (5_000_000, 0.0, Easing::Linear),
        ]);
        let cut = layer.sliced(500_000, 3_000_000);
        let times: Vec<u64> = cut.keyframes.iter().map(|k| k.time_us).collect();
        // Edges at 0 and 2.5 s, plus the two interior keyframes rebased.
        assert_eq!(times, vec![0, 500_000, 1_500_000, 2_500_000]);
        // The interior keyframe's own easing rides along.
        assert_eq!(cut.keyframe_at(500_000).unwrap().easing, Easing::Hold);
        // The `Hold` cut still happens at the same musical moment.
        assert!(close(cut.transform_at(1_400_000).x, 1.0));
        assert!(close(cut.transform_at(1_500_000).x, -1.0));
    }

    #[test]
    fn sliced_handles_empty_and_degenerate_ranges() {
        let still = BackgroundImage::new("bg1", "bg.png");
        assert!(still.sliced(0, 1_000_000).keyframes.is_empty());
        let layer = layer_with(&[(0, 0.0, Easing::Linear)]);
        assert!(layer.sliced(1_000_000, 1_000_000).keyframes.is_empty());
        assert!(layer.sliced(2_000_000, 1_000_000).keyframes.is_empty());
    }

    // ── BackgroundStack ─────────────────────────────────────────────────────

    fn stack_of(n: usize) -> BackgroundStack {
        let mut stack = BackgroundStack::new();
        for i in 0..n {
            stack.push(BackgroundImage::new(format!("bg{i}"), format!("bg{i}.png")));
        }
        stack
    }

    #[test]
    fn empty_stack_has_no_selection() {
        let stack = BackgroundStack::new();
        assert!(stack.is_empty());
        assert_eq!(stack.len(), 0);
        assert_eq!(stack.selected_index(), None);
        assert!(stack.selected().is_none());
        assert!(stack.views_at(0).is_empty());
    }

    #[test]
    fn push_selects_the_new_layer() {
        let mut stack = BackgroundStack::new();
        assert_eq!(stack.push(BackgroundImage::new("a", "a.png")), 0);
        assert_eq!(stack.selected_index(), Some(0));
        assert_eq!(stack.push(BackgroundImage::new("b", "b.png")), 1);
        assert_eq!(stack.selected_index(), Some(1));
        assert_eq!(stack.selected().unwrap().id, "b");
    }

    #[test]
    fn cycle_wraps_in_both_directions() {
        let mut stack = stack_of(3);
        stack.select(0);
        stack.cycle(-1);
        assert_eq!(stack.selected_index(), Some(2));
        stack.cycle(1);
        assert_eq!(stack.selected_index(), Some(0));
        stack.cycle(5);
        assert_eq!(stack.selected_index(), Some(2));
        // Empty stack: cycling is a no-op, not a panic.
        let mut empty = BackgroundStack::new();
        empty.cycle(1);
        assert_eq!(empty.selected_index(), None);
    }

    #[test]
    fn select_rejects_an_out_of_range_index() {
        let mut stack = stack_of(2);
        assert!(stack.select(1));
        assert_eq!(stack.selected_index(), Some(1));
        assert!(!stack.select(7));
        assert_eq!(stack.selected_index(), Some(1), "selection unchanged");
    }

    #[test]
    fn remove_keeps_the_selection_in_range() {
        let mut stack = stack_of(3);
        stack.select(2);
        assert_eq!(stack.remove(2).unwrap().id, "bg2");
        assert_eq!(stack.selected_index(), Some(1));
        assert!(stack.remove(9).is_none());
        assert_eq!(stack.remove_by_id("bg0").unwrap().id, "bg0");
        assert!(stack.remove_by_id("bg0").is_none());
        assert_eq!(stack.len(), 1);
        assert_eq!(stack.selected_index(), Some(0));
    }

    #[test]
    fn views_at_evaluates_each_layer_and_marks_the_selection() {
        let mut stack = BackgroundStack::new();
        let mut moving = BackgroundImage::new("bg0", "bg0.png");
        moving.set_keyframe(0, Transform::new(0.0, 0.0, 1.0, 0.0, 1.0), Easing::Linear);
        moving.set_keyframe(
            1_000_000,
            Transform::new(1.0, 0.0, 1.0, 0.0, 1.0),
            Easing::Linear,
        );
        stack.push(moving);
        stack.push(BackgroundImage::new("bg1", "bg1.png"));
        stack.select(0);

        let views = stack.views_at(500_000);
        assert_eq!(views.len(), 2);
        assert_eq!(views[0].index, 0);
        assert_eq!(views[0].file, "bg0.png");
        assert!(views[0].selected);
        assert!(close(views[0].transform.x, 0.5));
        assert_eq!(views[0].keyframes.len(), 2);
        assert!(!views[1].selected);
        assert_eq!(views[1].transform, Transform::IDENTITY);
    }

    #[test]
    fn from_layers_normalizes_and_round_trips_through_into_layers() {
        let mut wild = BackgroundImage::new("bg0", "bg0.png");
        wild.keyframes = vec![
            Keyframe::new(2_000_000, Transform::IDENTITY, Easing::Linear),
            Keyframe::new(1_000_000, Transform::IDENTITY, Easing::Linear),
        ];
        let stack = BackgroundStack::from_layers(vec![wild]);
        assert_eq!(stack.selected_index(), Some(0));
        let layers = stack.into_layers();
        let times: Vec<u64> = layers[0].keyframes.iter().map(|k| k.time_us).collect();
        assert_eq!(times, vec![1_000_000, 2_000_000]);
    }

    #[test]
    fn stack_shift_keyframes_moves_every_layer() {
        let mut stack = BackgroundStack::new();
        stack.push(layer_with(&[(1_000_000, 0.0, Easing::Linear)]));
        stack.push(layer_with(&[(2_000_000, 0.0, Easing::Linear)]));
        stack.shift_keyframes(-1_000_000);
        assert_eq!(stack.layers()[0].keyframes[0].time_us, 0);
        assert_eq!(stack.layers()[1].keyframes[0].time_us, 1_000_000);
    }

    #[test]
    fn lookup_helpers_find_layers_by_id() {
        let mut stack = stack_of(2);
        assert!(stack.contains_id("bg1"));
        assert!(!stack.contains_id("nope"));
        stack
            .get_by_id_mut("bg1")
            .expect("layer exists")
            .set_keyframe(0, Transform::IDENTITY, Easing::Linear);
        assert_eq!(stack.layers()[1].keyframes.len(), 1);
        assert!(stack.get_by_id_mut("nope").is_none());
    }

    // ── serde ───────────────────────────────────────────────────────────────

    #[test]
    fn layer_round_trips_through_json() {
        let layer = layer_with(&[(0, 0.0, Easing::EaseInOut), (1_000_000, 1.0, Easing::Hold)]);
        let json = serde_json::to_string(&layer).unwrap();
        let back: BackgroundImage = serde_json::from_str(&json).unwrap();
        assert_eq!(layer, back);
    }

    #[test]
    fn keyframe_easing_defaults_when_absent() {
        // A hand-written keyframe with no `easing` key is linear.
        let k: Keyframe = serde_json::from_str(
            r#"{"time_us":0,"transform":{"x":0.0,"y":0.0,"scale":1.0,"rotation_deg":0.0,"opacity":1.0}}"#,
        )
        .unwrap();
        assert_eq!(k.easing, Easing::Linear);
        // …and a layer with no `keyframes` key is a still backdrop.
        let l: BackgroundImage = serde_json::from_str(r#"{"id":"a","file":"a.png"}"#).unwrap();
        assert!(l.keyframes.is_empty());
        assert_eq!(l.transform_at(1_234).x, 0.0);
    }

    #[test]
    fn easing_serialises_snake_case_matching_its_name() {
        for e in [
            Easing::Linear,
            Easing::EaseIn,
            Easing::EaseOut,
            Easing::EaseInOut,
            Easing::Hold,
        ] {
            let json = serde_json::to_string(&e).unwrap();
            assert_eq!(json, format!("\"{}\"", e.name()));
            let back: Easing = serde_json::from_str(&json).unwrap();
            assert_eq!(e, back);
        }
    }
}
