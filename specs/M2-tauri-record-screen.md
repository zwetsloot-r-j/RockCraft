# M2-tauri-record-screen — Record screen, Design E "Studio (full + notation)"

> Milestone: M2 · Issue: #24 · Suggested tier: sonnet
> Branch: `claude/tauri-record-screen`
> Depends on: M2-tauri-scaffold (#22, **merged**) **and M7-tauri-0-solid-swap
> (#171 — the frontend is SolidJS; do not start before it lands)**
> Follow-up: M7-tauri-I-record-live (#169) wires this screen to live MIDI + saving

## Goal

Implement the **Studio (full + notation)** record screen (Design E from
`design/record_screen/`) inside `tauri-app/`. The screen shows a live recording
session: notes rise from the keyboard as you play, crystallise into a grand
staff above, with a full transport/edit toolbar and a selected-note inspector.
All data is mocked from the Ember Lantern take fixture; live MIDI hookup is a
follow-up.

## Context

### How this relates to the live TUI (updated 2026-06)

This spec predates the TUI record screen's current state. The mock-only scope
below is **unchanged**, but know what the controls will eventually mean so
the port stays wirable (#169, `specs/M7-tauri-I-record-live.md`):

- **Real today** in core/TUI (`crates/tui/src/record.rs`, `core::Composer`):
  recording to an `EventBuffer`, saving bundles to
  `recordings/take-<timestamp>/` (`song.mid` + `meta.json` + backing copy),
  recording **with a backing track** (origin anchored to backing start),
  metronome (`toggle_metronome`), count-in (`start_count_in_record`),
  undo/redo.
- **Visual-only stubs with no core action yet**: Trim, Quantize, Punch-in,
  the CLEF/SPELL pickers, and the note inspector's Nudge/Snap buttons. Build
  them as the spec says, but keep them clearly separable — #169 disables the
  unwired ones rather than faking behaviour.
- The SNAP picker below shows `1/8 · 1/16 · 1/32`; core's
  `Subdivision` also has `Quarter` and two triplet values
  (`crates/core/src/grid.rs`). Keep the segmented control's value list in
  one constant so #169 can extend it without layout surgery.
- The app shell/router lands separately (#162). If present on `main`, mount
  this as the `record` route instead of hand-rolling the two-button switcher
  in `App.tsx`.

Design reference files:

- `design/record_screen/rockcraft-proto/highway.js` — shared key-layout / rendering utils
- `design/record_screen/rockcraft-proto/record.js` — `RecordCanvas` engine (rising ribbons + staff)
- `design/record_screen/rockcraft-proto/record-ui.jsx` — shared UI primitives (`RUI` namespace)
- `design/record_screen/rockcraft-proto/record-protos.jsx` — `RecStudioFull` component (Design E)
- `design/record_screen/screenshots/vE.png` / `vE2.png` / `vE-hdr.png` — visual target

### Design E layout (top → bottom)

**Top status bar** (56 px, `#181a24`):
- Logo badge (two spectrum-colored bars)
- Song title + take number; key / BPM / time-sig subtitle
- Vertical rule
- `RecDot` (animated red orb when recording)
- Timecode (`MM:SS.mmm` monospace)
- Bar:beat chip
- Chord chip
- flex-1
- Metronome toggle (icon + BPM value)
- Count-in toggle (`COUNT 1`)
- MIDI device chip (green dot + device name)
- Level meter (vertical bars)

**Canvas area** (flex 1):
- `RecordCanvas` with `viz: "ribbons+staff"` — bottom half rising ribbons,
  top half grand staff with note heads that appear as ribbons cross the staff
- **Selected-note inspector** (absolute, top-right, 184 px wide): appears when
  a note is clicked; shows note name (colored pill), Beat, Length, Velocity;
  Nudge and Snap buttons

**Bottom toolbar** (58 px, `#181a24`):
- Transport: rewind, stop (red), play, loop
- Vertical rule
- Edit tools: Trim, Delete (danger), Quantize, Punch-in, Undo, Redo
- flex-1
- Segmented picker: `CLEF` (Grand / Treble)
- Segmented picker: `SPELL` (♯ / ♭)
- Vertical rule
- Segmented picker: `SNAP` (1/8 / 1/16 / 1/32)

## What to do

### File layout (inside `tauri-app/src/`)

```
screens/
  record/
    RecordScreen.tsx      # top-level layout: status bar + canvas + toolbar
    RecordHeader.tsx      # top status bar (56 px)
    RecordToolbar.tsx     # bottom edit/transport bar (58 px)
    RecordCanvas.ts       # port of record.js RecordCanvas (TypeScript class)
    NoteInspector.tsx     # selected-note floating panel (absolute positioned)
    types.ts              # RecordConfig, TakeNote, RecordEngine state types
    song.ts               # Ember Lantern "take" fixture (ported from record.js)
    ui/
      RoundBtn.tsx        # icon button (rewind / stop / play / loop)
      ToolBtn.tsx         # icon + label toolbar button
      Chip.tsx            # status chip
      Toggle.tsx          # metronome / count-in toggle
      Seg.tsx             # segmented control (CLEF, SNAP, SPELL)
      Meter.tsx           # vertical level meter
      RecDot.tsx          # animated recording orb
      VRule.tsx           # 1 px vertical divider
```

### `RecordCanvas.ts`

Port `RecordCanvas` from `record.js` as a TypeScript class:
- `constructor(canvas, config, song)` — initialise engine
- `start()` / `stop()` — RAF loop
- `cfg.paused: boolean` — pause/resume
- Read properties: `now` (ms elapsed), `level` (0–1 meter value), `sel`
  (selected note or null)
- Renders rising ribbon notes from the keyboard hit-line upward (opposite
  direction to the note highway)
- `viz: "ribbons+staff"` — in the upper portion of the canvas, draws a grand
  staff with note heads placed at the correct staff position as ribbons reach
  that region
- Spectrum color scheme (same `oklch` pitch-class wheel as the highway)

### `RecordScreen.tsx`

```tsx
export function RecordScreen() {
  let canvasEl!: HTMLCanvasElement;
  let eng: RecordCanvas | null = null;
  const [frame, setFrame] = createSignal(0); // bumped ~9 fps for header/toolbar reads
  const [metro, setMetro] = createSignal(true);
  const [count, setCount] = createSignal(true);
  const [snap, setSnap] = createSignal<"1/8" | "1/16" | "1/32">("1/16");
  const [clef, setClef] = createSignal<"Grand" | "Treble">("Grand");
  const [spelling, setSpelling] = createSignal<"♯" | "♭">("♯");
  // onMount: create engine on canvasEl, eng.start(), ~110 ms interval bumping
  // frame(); onCleanup: eng.stop(). Pass signal accessors down (metro, not
  // metro()) so the chrome stays reactive — see tauri-app/CONVENTIONS.md.
  return (
    <div style={{ height: "100vh", display: "flex", "flex-direction": "column",
                  background: "#101119", "font-family": "'Space Grotesk', system-ui, sans-serif" }}>
      <RecordHeader eng={() => eng} frame={frame} song={RSONG}
                    metro={metro} onMetro={setMetro}
                    count={count} onCount={setCount} />
      <div style={{ flex: "1 1 auto", "min-height": 0, position: "relative" }}>
        <canvas ref={canvasEl} style={{ width: "100%", height: "100%", display: "block" }} />
        <NoteInspector eng={() => eng} frame={frame} />
      </div>
      <RecordToolbar snap={snap} onSnap={setSnap} clef={clef} onClef={setClef}
                     spelling={spelling} onSpelling={setSpelling} />
    </div>
  );
}
```

### UI primitives

Port each component from `record-ui.jsx` (`RUI` namespace) to a typed Solid
component (the source is React JSX — translate per
`tauri-app/CONVENTIONS.md`; never destructure props). Props must be fully
typed. All toggle/segmented controls must be interactive (local state changes
visually; they do not need to affect the canvas engine for this issue).

### `App.tsx`

Add a minimal screen switcher (two buttons: "Highway" / "Record") so both
screens are reachable without routing infrastructure. The highway screen should
still be accessible if both issues land.

### Shared utilities

If `HighwayCanvas.ts` is already present (highway issue merged), import
`keyLayout`, `withAlpha`, `shade`, `spectrumHue` from
`screens/highway/utils.ts` instead of duplicating them. If not yet merged,
duplicate them into `screens/record/utils.ts`.

## Tests

No automated tests required for this visual screen. Manual acceptance criteria
below.

## Scope boundaries (do NOT)

- Do not wire to live Tauri IPC / real MIDI — mock take fixture only
  (live wiring is #169; the IPC bridge itself is #161).
- Do not implement the note highway (separate issue).
- Do not modify anything in `crates/`.
- Do not implement drag-edit of notes on the canvas (inspector Nudge/Snap
  buttons are purely visual stubs).
- Do not add audio or metronome sound output.

## Acceptance

- [ ] `npx tsc --noEmit` passes
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green
- [ ] Running `npm run dev` shows the record screen with animated rising ribbons
      and a grand staff appearing above
- [ ] Top status bar shows timecode, bar:beat, chord, MIDI device chip, meter
- [ ] Metronome and count-in toggles respond visually
- [ ] SNAP, CLEF, SPELL segmented controls respond visually
- [ ] Selected-note inspector appears (click or auto-select on a note in the mock)
- [ ] PR opened against `main` from `claude/tauri-record-screen`, `Closes #24`
