# M2-tauri-note-highway — Note highway screen, Design D "Spectrum Live"

> Milestone: M2 · Issue: #23 · Suggested tier: sonnet
> Branch: `claude/tauri-note-highway`
> Depends on: M2-tauri-scaffold (#22 must be merged first)

## Goal

Implement the **Spectrum Live** note-highway screen (Design D from
`design/note_highway/`) inside `tauri-app/` as a fully animated React screen.
The canvas engine is ported from the design prototype; data is supplied by a
mock song fixture (live core integration is a follow-up).

## Context

Design reference files:

- `design/note_highway/rockcraft-proto/highway.js` — canvas engine (`HighwayCanvas`)
- `design/note_highway/rockcraft-proto/prototypes.jsx` — `FusionProto` component (Design D)
- `design/note_highway/rockcraft-proto/song.js` — song fixture (Ember Lantern)
- `design/note_highway/screenshots/01-fusion.png` / `02-fusion.png` — visual target

Design D config (`cfgFusion`):
```
colorMode: "spectrum"   pitchRuler: true    bg: "#0f1016"
perspective: 0          kbRatio: 0.2        hitLine: "#aab2d0"
glow: 0.32              lead: 3000 ms       noteGap: 0.2
gridlines: "soft"       scoring: true       radius: 3
keyboard: "flat"        labels: true
```

Header layout (left→right): song title + key badge + 12-dot color wheel |
flex-1 | bar:beat display | chord display | vertical rule | combo multiplier
(×N, with `COMBO` sub-label) | score (6-digit padded, with `SCORE` sub-label).
Fonts: `Space Grotesk` (display) and `IBM Plex Mono` (mono). Background `#181922`,
bottom border `rgba(255,255,255,0.07)`, height 64 px.

## What to do

### File layout (inside `tauri-app/src/`)

```
screens/
  highway/
    HighwayScreen.tsx     # top-level component; mounts header + canvas
    HighwayHeader.tsx     # the 64 px header bar
    HighwayCanvas.ts      # port of highway.js HighwayCanvas (TypeScript class)
    types.ts              # NoteSpan, SongData, HighwayConfig, KeyLayout, etc.
    song.ts               # Ember Lantern mock fixture (ported from song.js)
    utils.ts              # keyLayout(), roundRect(), withAlpha(), shade(), spectrumHue()
```

### `types.ts`

```typescript
export interface NoteSpan { note: number; start: number; end: number; hand: "L" | "R"; }
export interface SongData {
  title: string; artist: string; key: string; timeSig: string; tempoBpm: number;
  notes: NoteSpan[]; chords: string[];
  LOOP: number; BEAT: number; BAR: number;
}
export interface HighwayConfig {
  lead?: number; bg?: string; kbRatio?: number; noteGap?: number; radius?: number;
  colorMode?: "hands" | "spectrum" | "accent";
  handColors?: { L: string; R: string };
  perspective?: number; glow?: number; labels?: boolean;
  gridlines?: "none" | "soft"; keyboard?: "realistic" | "flat" | "strip";
  hitLine?: string; scoring?: boolean; pitchRuler?: boolean;
  laneTint?: string;
}
export interface KeyInfo { note: number; x: number; w: number; cx: number;
                            black: boolean; wi?: number; }
export interface KeyLayout { whites: KeyInfo[]; blacks: KeyInfo[];
                              byNote: Record<number, KeyInfo>; }
```

### `HighwayCanvas.ts`

Port `HighwayCanvas` from `highway.js` as a TypeScript class with the same
public API: `constructor(canvas, config, song)`, `start()`, `stop()`, read
properties `score`, `combo`, `hits`, `total`, `t0`. Keep all drawing methods
(`drawBackground`, `drawGrid`, `drawNotes`, `drawHitLine`, `drawKeyboard`,
`drawRuler`, `drawScoreFx`, `runScoring`). Replace `window.RC.*` globals with
constructor parameters.

### `HighwayScreen.tsx`

```tsx
export function HighwayScreen() {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const engRef = useRef<HighwayCanvas | null>(null);
  const [, forceUpdate] = useReducer((x) => x + 1, 0);
  // mount engine, RAF for header state updates (throttled ~9 fps)
  // unmount: eng.stop()
  return (
    <div style={{ height: "100vh", display: "flex", flexDirection: "column",
                  background: "#0f1016", fontFamily: "'Space Grotesk', system-ui, sans-serif" }}>
      <HighwayHeader engRef={engRef} song={SONG} />
      <div style={{ flex: "1 1 auto", minHeight: 0, position: "relative" }}>
        <canvas ref={canvasRef} style={{ width: "100%", height: "100%", display: "block" }} />
      </div>
    </div>
  );
}
```

### `HighwayHeader.tsx`

Faithfully reproduce the `FusionProto` header from the design. Reads
`engRef.current` for `score`, `combo`; derives bar/beat/chord from
`performance.now() - eng.t0` modulo `SONG.LOOP`. The 12-dot color wheel uses
`oklch(0.72 0.16 ${(i * 30 + 8) % 360})` for i in 0..11, rendered as 7×7 px
rounded squares.

### `App.tsx`

Replace the placeholder with `<HighwayScreen />` (or add a minimal route if a
record screen is already present from a sibling PR — coordinate via branch).

### Fonts

Add to `index.html`:
```html
<link rel="preconnect" href="https://fonts.googleapis.com" />
<link href="https://fonts.googleapis.com/css2?family=IBM+Plex+Mono:wght@400;500;600&family=Space+Grotesk:wght@400;600;700&display=swap" rel="stylesheet" />
```

## Tests

No automated tests required for this visual/canvas screen. Manual acceptance
criteria are listed below. The port must compile and the canvas must animate
before PR is opened.

## Scope boundaries (do NOT)

- Do not wire to live Tauri IPC / real MIDI data — mock song only.
- Do not implement the record screen (separate issue).
- Do not modify anything in `crates/`.
- Do not add audio playback.
- Do not add a settings panel or theme switcher.

## Acceptance

- [ ] `npx tsc --noEmit` passes
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean
- [ ] `cargo test --workspace` green
- [ ] Running `npm run dev` inside `tauri-app/` opens the window and the note
      highway animates (notes fall, keyboard lights, score/combo update)
- [ ] Header shows bar:beat, chord, combo, score, color wheel
- [ ] Pitch ruler and note-name labels visible on the canvas
- [ ] PR opened against `main` from `claude/tauri-note-highway`, `Closes #23`
