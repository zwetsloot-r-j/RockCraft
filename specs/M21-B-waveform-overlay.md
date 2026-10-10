# M21-B — Waveform overlay in the desktop edit grid

> Milestone: M21 — Audio-guided editing · Issue: TBD · Suggested tier: sonnet
> Branch: `claude/m21-waveform-overlay`
> **Depends on M21-A** — `backing_waveform` / `editBackingWaveform()` must be on `main`.

## Goal

Draw the backing track's two curves from M21-A in the desktop edit screen,
time-aligned with the grid. They sit over the background movie but behind
the grid lines and notes. Each is a thin strip along a window edge, so the
centre of the movie stays clear:

- **Envelope (loudness)**: a strip on the **right** edge; bars grow leftward.
- **Onsets (note/hit starts)**: a strip on the **left** edge; bars grow rightward.

## Context

- Renderer: `tauri-app/src/screens/edit/EditCanvas.ts`, `draw()`. Order today:
  backdrop frame + dim fill (`fillRect` right after the backdrop block), then
  `drawLanes`, `drawLoopRegion`, `drawGridlines`, … notes … keyboard strip.
- Time → screen: `Viewport.yOf(us)` (`viewport.ts`; later time is higher up),
  the same viewport that places notes and grid lines.
- Song time `t` maps to backing-file position `t + snapshot.backing_offset_us`.
  The `waveform` data is indexed by file position (bucket
  `floor((t + offset) / bucket_us)`). Nudging the offset
  (`nudge_backing_offset`) must move the strips immediately, without a refetch.
- `EditScreen.tsx` owns backing state (`backingName`, attach/detach via `B`),
  per-view prefs in `localStorage` (e.g. `rockcraft.videocal:`), and the
  status-bar hint line (`StatusBar.tsx`).
- Free keys in the edit screen at the time of writing include `O`. Pick another
  if it has since been taken, and note which in the PR.

## What to do

### 1. Fetch and hold the data (`EditScreen.tsx`)

- After a bundle loads and after `B` attaches/replaces a backing, call
  `editBackingWaveform()`. While it answers `pending`, poll every 500 ms
  (stop on `ready`/`none`, on detach, and on unmount). On detach, clear it.
- Pass the result to the canvas: `engine.setWaveform(w | null)`.

### 2. Draw (`EditCanvas.ts`)

- New `drawWaveform(snapshot, vp)`, called **after `drawLanes` and before
  `drawGridlines`**. That puts it above the movie and lanes, below the grid
  lines, notes, cursor and keyboard strip. (Loop-region tint may go before or
  after it; keep the loop region legible.)
- For each 1-px row between the top of the canvas and the top of the keyboard
  strip: find the song-time span the row covers, map it to buckets, and take
  the **max** value over those buckets (so short spikes never disappear when
  zoomed out). Skip rows before file position 0 or past the data.
- Strip width: **12 %** of the canvas width each, bar length
  `value / 255 × stripWidth`.
  - Envelope: right-aligned at `x = w`, drawn leftward.
  - Onsets: left-aligned at `x = 0`, drawn rightward.
- Colours: translucent, distinct, and calm over a movie, e.g. envelope
  `rgba(120,180,255,0.35)`, onsets `rgba(255,190,90,0.45)`. Define them as
  named constants beside the canvas's other colours.
- Draw nothing when there is no waveform or the overlay is off.
- Cheap enough per frame: at most two `fillRect` per row; no allocation in
  the loop.

### 3. Toggle

- `O` cycles **both → envelope only → onsets only → off → both**. Default:
  both. Persist the choice in `localStorage` (`rockcraft.waveform`), wrapped in
  try/catch as the other prefs are.
- Status bar: show `wave both|env|onset|off` while a backing is attached, and
  add `O wave` to the hint line. When the data is `pending`, show `wave …`.
- Add the key to the edit help overlay (`?`).

## Tests

- Unit (vitest, pure helper extracted from the draw code, e.g.
  `waveform.ts`): row → bucket range mapping, including a non-zero
  `backing_offset_us` (positive and negative), max-pooling over several
  buckets per row, and rows outside the data returning nothing.
- Unit: the toggle cycle and its persistence fallback when `localStorage`
  throws.
- `npx tsc --noEmit` and the existing vitest suite stay green.

## Manual check (local)

Load an imported piece (it has `backing.wav`), open the editor: the right
strip shows the loudness and the left strip the onsets, scrolling exactly with
the grid. Drum hits line up with spikes on both. Nudging the backing offset
moves both strips. `O` cycles the modes, and the choice survives a restart.

## Out of scope

- TUI rendering.
- Play/highway screen overlay.
- Finer-than-10 ms detail or smoothing.
