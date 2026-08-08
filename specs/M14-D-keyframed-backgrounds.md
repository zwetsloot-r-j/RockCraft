# M14-D — edit/play: custom background images with keyframed transforms

> Milestone: M14 — Play-screen polish · Issue: #260 · Suggested tier: opus
> Branch: `claude/issue-260-2e50o7`

## Goal

Let a piece carry its own **background images** — attach one or more in edit
mode, lay each out (position / scale / rotation / opacity), and drop
**keyframes** on the timeline so the layout *interpolates* while the song plays.
An animated backdrop authored entirely inside RockCraft, next to the existing
movie backdrop rather than instead of it.

`core` owns the model and the interpolation math (pure, headless-testable); the
webview only renders the transform it is handed.

## Context

- The piece already carries **one** `BackgroundVideo` (`core::song`, M9-G) —
  a bundle-relative `file` plus an `offset_us`. Background images are the
  sibling concept: a *list*, each with a per-time transform instead of a time
  offset. They compose — a piece can have the movie *and* images.
- The Tauri `AttachedVideo` (`tauri-app/src-tauri/src/state.rs`) is the exact
  template for the frontend-mirrored, copied-into-the-bundle file reference.
- `Composer::playhead_us()` already means "transport position while playing,
  cursor position while stopped". That is the **edit time** keyframing works at,
  so no new notion of "now" enters the composer.
- Transform values must survive `Action`'s `Eq` derive, so every background
  action carries **integers** (permille / millidegrees), mirroring
  `Action::SetPlaybackRate { rate_permille }`.

## What to do

### D1 — `core` model + interpolation (`crates/core/src/background.rs`, new)

```rust
pub struct Transform { x: f32, y: f32, scale: f32, rotation_deg: f32, opacity: f32 }
```

Normalised, resolution-independent surface units so the webview can apply it
verbatim: `x`/`y` are the image centre's offset from the surface centre in
surface widths/heights, `scale` multiplies a `object-fit: contain` fit,
`rotation_deg` is clockwise, `opacity` is `0..=1`. `Transform::IDENTITY` is the
untouched, centred, fully opaque image.

- `Transform::new(..)` / `clamped()` clamp to `±4.0` (position), `0.05..=10.0`
  (scale), `0.0..=1.0` (opacity), and wrap rotation into `(-180, 180]`.
  Non-finite inputs fall back to the identity component (never `NaN`).
- `Transform::lerp(a, b, t)` interpolates each component linearly with `t`
  clamped to `0..=1`, taking rotation along the **shortest arc** (170° → -170°
  travels 20°, not 340°).

```rust
pub enum Easing { Linear, EaseIn, EaseOut, EaseInOut, Hold }
pub struct Keyframe { time_us: u64, transform: Transform, easing: Easing }
pub struct BackgroundImage { id: String, file: String, keyframes: Vec<Keyframe> }
pub struct BackgroundStack { layers: Vec<BackgroundImage>, selected: usize }
pub struct BackgroundView { index, id, file, selected, transform, keyframes }
```

- `Easing::apply(t)` shapes the segment parameter; it is the easing of the
  keyframe the segment **leaves**. `Hold` is a cut (stays on the earlier
  keyframe until the later one is reached).
- `BackgroundImage::transform_at(us)`: no keyframes → `IDENTITY`; before the
  first / after the last → that keyframe's transform (no extrapolation);
  between → `lerp` under the earlier keyframe's easing.
- `set_keyframe(time_us, transform, easing)` replaces an existing keyframe at
  exactly `time_us`, else inserts it in sorted order. `remove_keyframe_at`,
  `set_easing_at`, `keyframe_at`, `shift_keyframes(delta_us)` (saturating, for
  segment slicing), `normalize()` (sort + de-duplicate + clamp after
  deserialisation).
- `BackgroundStack` owns the layer list plus the **selected index** the actions
  address: `select`, `cycle(delta)` (wrapping), `push` (selects the new layer),
  `remove` / `remove_by_id` (selection stays in range), `views_at(us)`.

### D1b — persistence + snapshot

- `RecordingMeta.backgrounds: Vec<BackgroundImage>`, `#[serde(default)]` so
  every legacy bundle still parses.
- `ComposerSnapshot.backgrounds: Vec<BackgroundView>` (transform already
  evaluated at `playhead_us()`) and `selected_background: Option<usize>`.
- `core::segment::slice_segment` shifts keyframe times with the rest of the
  piece's media so a split keeps the animation aligned.

### D2 — edit-mode layout + keyframing (`core::Action`, pure)

Auto-keyframing, After-Effects style: any nudge writes the keyframe at the edit
time (`playhead_us()`), **creating** it from the currently interpolated
transform when none sits exactly there.

| action | params | effect |
| --- | --- | --- |
| `select_background` | `index: u32` | address layer `index` |
| `cycle_background` | `delta: i32` | wrap the selection by `delta` |
| `nudge_background_pos` | `dx_permille: i32`, `dy_permille: i32` | move |
| `nudge_background_scale` | `delta_permille: i32` | zoom |
| `nudge_background_rotation` | `delta_millideg: i32` | rotate |
| `set_background_opacity` | `permille: u16` | fade |
| `set_background_easing` | `easing: Easing` | curve leaving this keyframe |
| `add_background_keyframe` | — | pin the interpolated transform here |
| `delete_background_keyframe` | — | drop the keyframe at the edit time |

All are no-ops (never errors) when the piece has no background layers.

### D2b — attach/detach (`control::HostCommand`, I/O)

Copying a file into a bundle is I/O, so it cannot be a `core::Action`:

- `attach_background { path: String }` → add a layer, return the layer list.
- `detach_background { id: String }` → remove that layer.
- `query_backgrounds` → the layers with their current transforms.

Wired into the Tauri `HostServices` match (real) and the TUI's (`Unsupported`,
like `attach_video` — a terminal cannot draw an image).

### D3 — rendering (Tauri webview)

One absolutely-positioned `<img>` per layer, `object-fit: contain`, behind the
grid/highway canvas and behind the movie backdrop's z-order neighbours, driven
by the **core-evaluated** transform:
`translate(x·W, y·H) rotate(deg) scale(s)` + `opacity`. No interpolation math in
TypeScript.

- **Edit screen**: layers follow the composer snapshot, so scrubbing the cursor
  or playing back animates them live. Keys: `Ctrl+Shift+←/→/↑/↓` move,
  `Ctrl+Shift+-/=` scale, `[` / `]` rotate, `Ctrl+Shift+K` keyframe,
  `Ctrl+Shift+Backspace` delete keyframe, `Ctrl+Shift+B` cycle layer.
- **Play screen**: `play_load` returns the layers, `play_state` carries the
  transforms evaluated each tick against the play clock.

## Tests

- `Transform`: clamping (position/scale/opacity), rotation wrapping, non-finite
  → identity component; `lerp` at `t = 0 / 0.5 / 1`; shortest-arc rotation
  across the ±180 seam; `t` outside `0..=1` clamped.
- `Easing`: each variant at `t = 0 / 0.5 / 1`; `Hold` stays at 0 until `t = 1`.
- `transform_at`: empty → identity; single keyframe → constant; before first /
  after last → clamped; midpoint of two → exact halfway values; eased segment
  ≠ linear segment.
- `set_keyframe` inserts sorted, replaces at an exact time; `remove_keyframe_at`
  returns whether it removed; `normalize` sorts and de-duplicates.
- `BackgroundStack`: `cycle` wraps both directions, `remove` keeps the selection
  in range, `push` selects the new layer.
- `RecordingMeta` round-trips with and without `backgrounds`; a legacy
  `meta.json` with no `backgrounds` key parses to an empty list.
- `Composer`: a nudge with no layers is a no-op; a nudge creates the keyframe at
  `playhead_us()` seeded from the interpolated transform; the snapshot's
  transform tracks the playhead; `delete_background_keyframe` removes only the
  one at the edit time.
- `action.rs` / `host.rs` parity tests cover the new variants automatically.
- `slice_segment` shifts keyframes and drops the piece's lead-in.
- Vitest: the CSS transform string built from a `Transform`.

## Scope boundaries (do NOT)

- Do not add third-party dependencies (Rust or npm).
- Do not touch the existing `BackgroundVideo` model, its offset nudging, or the
  edit-screen video calibration — images are an independent layer set.
- Do not put interpolation math in TypeScript; the webview renders what `core`
  computed.
- Do not add per-layer blend modes, cropping, z-order re-ordering, or image
  decoding/scaling in Rust.
- Do not render images in the TUI.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] `npm run check` (tsc + vitest) green in `tauri-app/`
- [ ] PR opened against `main` from the branch above, `Closes #260`
