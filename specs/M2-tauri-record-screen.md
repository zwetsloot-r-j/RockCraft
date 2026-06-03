# M2-tauri-record-screen — Record screen, Design E "Studio (full + notation)"

> Milestone: M2 · Issue: #24 · Suggested tier: sonnet
> Branch: `claude/tauri-record-screen`
> Depends on: M2-tauri-scaffold (#22 must be merged first)

## Goal

Implement the **Studio (full + notation)** record screen (Design E from
`design/record_screen/`) inside `tauri-app/`. The screen shows a live recording
session: notes rise from the keyboard as you play, crystallise into a grand
staff above, with a full transport/edit toolbar and a selected-note inspector.
All data is mocked from the Ember Lantern take fixture; live MIDI hookup is a
follow-up.

## Context

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
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engRef = useRef<RecordCanvas | null>(null);
  const [, tick] = useReducer((x) => x + 1, 0);
  const [metro, setMetro] = useState(true);
  const [count, setCount] = useState(true);
  const [snap, setSnap] = useState<"1/8" | "1/16" | "1/32">("1/16");
  const [clef, setClef] = useState<"Grand" | "Treble">("Grand");
  const [spelling, setSpelling] = useState<"♯" | "♭">("♯");
  // mount engine; RAF throttled at ~9 fps for header/toolbar state reads
  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column",
                  background: "#101119", fontFamily: "'Space Grotesk', system-ui, sans-serif" }}>
      <RecordHeader engRef={engRef} song={RSONG} metro={metro} onMetro={setMetro}
                    count={count} onCount={setCount} />
      <div style={{ flex: "1 1 auto", minHeight: 0, position: "relative" }}>
        <canvas ref={canvasRef} style={{ width: "100%", height: "100%", display: "block" }} />
        <NoteInspector note={engRef.current?.sel ?? null} />
      </div>
      <RecordToolbar snap={snap} onSnap={setSnap} clef={clef} onClef={setClef}
                     spelling={spelling} onSpelling={setSpelling} />
    </div>
  );
}
```

### UI primitives

Port each component from `record-ui.jsx` (`RUI` namespace) to a typed React
component. Props must be fully typed. All toggle/segmented controls must be
interactive (local state changes visually; they do not need to affect the canvas
engine for this issue).

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

- Do not wire to live Tauri IPC / real MIDI — mock take fixture only.
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
