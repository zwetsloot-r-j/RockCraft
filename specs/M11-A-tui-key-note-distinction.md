# M11-A — TUI highway: make white-key vs black-key note blocks distinct

> Milestone: M11 — Highway readability · Issue: #229 · Suggested tier: sonnet
> Branch: `claude/m11-tui-key-note-distinction`
> Related: M11-B (#230, same goal for the Tauri canvas highway)

## Goal

On the TUI note highway, make a note block for a **black key** (accidental:
C#/D#/F#/G#/A#) immediately recognizable as different from a **white key**
(natural) note block — at a glance, without counting columns. Today the only
difference is width (black = 1 column, white = full lane), which is easy to miss.

## Context

- Crate: `crates/tui` only. `core` carries no color/style — keep it that way.
- Highway rendering: `crates/tui/src/play.rs` — `draw_highway()` (~line 453).
  Current per-note logic:
  - Color: `TARGET_COLOR` (yellow) when the note is sounding *now*, otherwise
    `Color::Indexed(33)` (blue) for upcoming notes. This active/upcoming signal
    must be preserved.
  - Width: `let cell_w = if is_black_key(span.note) { 1 } else { w };` — black
    keys draw 1 column, white keys draw `w` columns (scale-dependent).
  - Glyph: `"▓"` for the block body.
- Key classifier: `crates/tui/src/keyboard.rs::is_black_key(note: u8) -> bool`
  (`matches!(note % 12, 1 | 3 | 6 | 8 | 10)`) — reuse it; do not duplicate.
- Existing palette constants in `crates/tui/src/render.rs`: `WHITE_KEY =
  Color::White`, `BLACK_KEY = Color::DarkGray` (used for the keyboard, not the
  highway today) — a sensible source of the white/black hue distinction.

## What to do

1. **Factor the per-note visual decision into a pure, testable helper** in
   `play.rs` (or `keyboard.rs`), e.g.:

   ```rust
   /// Visual style for one highway note block.
   pub struct NoteStyle { pub color: Color, pub glyph: char }
   /// `active` = the note is sounding at the current clock position.
   pub fn note_style(note: u8, active: bool) -> NoteStyle
   ```

   Behaviour:
   - The **active vs upcoming** distinction is preserved (active notes read as
     brighter / the existing `TARGET_COLOR`; upcoming as the existing blue).
   - **Layered on top**, black-key notes are visibly distinct from white-key
     notes via **two** redundant cues so it survives low-color terminals:
     (a) a different **shade/hue** (e.g. a dimmer/darker variant for accidentals),
     and (b) a different **fill glyph** (e.g. `▓` for white keys vs `▒`/`░` or a
     bordered glyph for black keys). Pick concrete values and document them.
   - Keep the existing **width** cue (black = 1 col) as the third cue.

2. **Use the helper in `draw_highway()`** so every drawn block goes through it.
   No behavioural change other than the new style.

3. Keep it legible against the highway background and the target line; verify
   visually in `--mock` mode.

## Tests

Unit tests on `note_style` (pure, no terminal needed):
- A black-key pitch (e.g. 61 = C#) and a white-key pitch (e.g. 60 = C) at the
  same `active` value return **different** `color` **and** different `glyph`.
- For a given pitch, `active = true` vs `false` differ (active/upcoming signal
  preserved).
- Spot-check the full octave: pitches `{1,3,6,8,10} mod 12` classify as black,
  the rest as white (i.e. the helper agrees with `is_black_key`).

## Scope boundaries (do NOT)

- Do **not** touch `core` or any other crate — `crates/tui` only.
- Do **not** change the highway geometry/scale, the keyboard rendering, or the
  active/upcoming color meaning beyond layering the key distinction onto it.
- Do **not** add third-party dependencies.
- Tauri parity is M11-B; do not edit `tauri-app/`.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] In `--mock` mode, black-key note blocks are clearly distinguishable from
      white-key blocks (shade + glyph + width), with active/upcoming still readable
- [ ] PR opened against `main` from the branch above, `Closes #229`
