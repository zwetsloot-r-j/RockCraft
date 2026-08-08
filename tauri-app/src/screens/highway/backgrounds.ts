// backgrounds.ts — rendering helpers for keyframed background images (M14-D).
//
// `core` owns the model *and* the interpolation math: every transform that
// reaches this file was already evaluated against the song clock in Rust
// (`background::BackgroundImage::transform_at`). Nothing here interpolates,
// eases, or reads a clock — these functions only turn a `BackgroundTransform`
// into the CSS an absolutely-positioned `<img>` needs, so the play screen and
// the edit screen paint layers identically.

import type { BackgroundTransform } from "../../ipc/types";

/** The transform an unkeyframed layer renders at: centred, contained, opaque. */
export const IDENTITY_TRANSFORM: BackgroundTransform = {
  x: 0,
  y: 0,
  scale: 1,
  rotation_deg: 0,
  opacity: 1,
};

/** Replace a non-finite number with `fallback` (a hand-edited bundle guard). */
function finite(value: number, fallback: number): number {
  return Number.isFinite(value) ? value : fallback;
}

/**
 * The CSS `transform` for one layer.
 *
 * `x`/`y` are fractions of the *surface*, and the element already fills the
 * surface (`inset: 0`), so a percentage translate is exactly right and stays
 * correct as the window resizes — no pixel measurement, no re-render on resize.
 * Order matters: translate, then rotate about the image centre, then scale.
 */
export function cssTransform(t: BackgroundTransform): string {
  const x = finite(t.x, 0) * 100;
  const y = finite(t.y, 0) * 100;
  const deg = finite(t.rotation_deg, 0);
  const scale = finite(t.scale, 1);
  return `translate(${x}%, ${y}%) rotate(${deg}deg) scale(${scale})`;
}

/** The CSS `opacity` for one layer, clamped into `0..=1`. */
export function cssOpacity(t: BackgroundTransform): number {
  return Math.min(1, Math.max(0, finite(t.opacity, 1)));
}

/**
 * The full inline style for a background `<img>`: it fills the surface, is
 * contained inside it, never takes pointer events, and carries the layer's
 * live transform. Layers paint back-to-front, so `index` becomes the z-order
 * *within* the background stack (still below the canvas, which sits higher).
 */
export function layerStyle(
  t: BackgroundTransform,
  index: number,
): Record<string, string> {
  return {
    position: "absolute",
    inset: "0",
    width: "100%",
    height: "100%",
    "object-fit": "contain",
    "pointer-events": "none",
    "z-index": String(index),
    "transform-origin": "center center",
    transform: cssTransform(t),
    opacity: String(cssOpacity(t)),
  };
}
