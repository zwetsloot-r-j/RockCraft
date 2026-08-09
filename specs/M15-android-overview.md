# M15 — Android: a play-only RockCraft

> Milestone: M15 — Android · Issue: #<n> · Suggested tier: opus
> Branch: n/a (umbrella spec — the sub-tasks carry their own branches)

## Goal

Ship RockCraft on Android as a **play-only** app: pick a piece from the library,
play it against a USB- or BLE-connected digital piano, get scored. No composing,
no recording, no import pipeline.

This spec is the umbrella: the architecture decisions, the task breakdown, and
the risks. Each sub-task gets its own `specs/M15-*.md` and its own issue.

## Why this is tractable

The workspace was already built for it. `core` (≈10k LOC) is pure domain with no
I/O, `crates/import`'s bundle *writer* is pure, `midly` file parsing is portable,
and `tauri-app/src-tauri/Cargo.toml` already declares
`crate-type = ["staticlib", "cdylib", "rlib"]` — with the comment *"so the same
`run()` can back both the desktop binary and future mobile entry points"*.

What does **not** port:

| Concern | Why | Resolution |
|---|---|---|
| Live MIDI input | `midir` 0.10 has no Android backend; an app cannot open ALSA | Kotlin Tauri plugin over `android.media.midi` → new `NoteSource` (M15-D) |
| Audio output | `rodio`/`cpal` *do* have an Android backend (oboe/AAudio), but it is the least-exercised one in the stack | Spike it early; M15-E |
| SoundFont | `crates/audio/assets/piano.sf2` is **32 MB**, loaded from a filesystem path | Compact sf2 shipped as an Android asset; `ROCKCRAFT_SF2` already overrides the path (M15-E) |
| Library location | `bundle::library_root()` returns a *relative* `library/`, and `default_scan_roots()` adds `recordings/` + `import-out/` | Set `ROCKCRAFT_LIBRARY_DIR` to the app-private dir at startup — the env seam already exists; the two missing roots are skipped harmlessly (M15-C) |
| Import pipeline | `crates/import/src/pipeline.rs` shells out to yt-dlp, ffmpeg, and a Python OMR sidecar | **Never ships on Android.** The phone consumes bundles; see "Getting songs onto the phone" |
| Edit / record | Not wanted | Absent from the mobile frontend; `HostServices` returns `Unsupported` |

## Decisions

### D1 — Tauri v2 Android, not native Kotlin and not Godot

Tauri v2 supports Android, and it is the only option that reuses both halves of
the existing app: the whole Rust stack *and* the SolidJS highway renderer
(`HighwayCanvas.ts` 829 LOC + `HighwayScreen.tsx` 673 LOC of canvas2d that
already draws the note highway, the backdrop, and the bottom keyboard).

The cost is that Tauri's Android support is younger than its desktop support and
we take on one custom mobile plugin (MIDI). Both are acceptable; **M15-0 gates
this decision** — if the WebView cannot hold frame rate, we revisit before any
other M15 work starts.

### D2 — Separate mobile crate, with the play engine extracted first

`mobile-app/src-tauri` is a **new crate** (`rockcraft-mobile`), a sibling of
`tauri-app/src-tauri`, added as a workspace member. Desktop is not touched by
mobile work and cannot be broken by it.

The direct consequence: anything both frontends need must be *extracted into
shared code*, not copied, or the two will drift the way
`tauri-app/src-tauri/src/play.rs` and `crates/tui/src/play.rs` already drift
(that file's own doc comment describes itself as being "at parity with the TUI
play screen" — parity maintained by hand).

So two extractions are prerequisites, not nice-to-haves:

- **Rust:** `PlaySession` and friends move out of
  `tauri-app/src-tauri/src/play.rs` (1763 LOC) into `crates/play` (M15-A). The
  file's own header says it is "pure state + plain serializable reports" with
  side effects applied by callers, and the filesystem reads sit in a separate
  loader (around line 1002), so the seam is already where it needs to be.
- **Frontend:** `tauri-app/src/screens/highway/*` and `tauri-app/src/ipc/*`
  become a shared npm workspace package consumed by both shells (M15-C).

`HostServices` keeps protecting us: the mobile impl's exhaustive `match` on
`HostCommand` must return `HostError::Unsupported` for every `Record*`,
`Import*`, `Split*`, and `Save*` variant, and the compiler will force every
*future* `HostCommand` to be consciously accepted or refused on mobile.

### D3 — Phone landscape is the design constraint

Target a 6" phone in landscape. 88 keys is not readable at that width, so the
visible key range is a **window** derived from the piece's actual pitch range
(the chart's min/max note plus a margin), not the full keyboard. Tablets simply
get a wider window from the same code path. Touch targets are sized for fingers
from day one. Details in M15-F.

### D4 — Default the player-notes synth OFF on Android

A physical gotcha, not a preference: the phone's only USB-C port is occupied by
the piano, so app audio leaves over the speaker or Bluetooth. BT audio latency
would smear the player's own synthesised notes against the sound the digital
piano is *already* making acoustically.

The piano is audible on its own, so mobile defaults the player bus to silent and
exposes a **calibration offset** for the song synth and backing track. A constant
offset is correctable; jitter is not. `core::Mixer` already owns per-bus gain, so
this is a default, not a new mechanism.

## Getting songs onto the phone

The chosen strategy is **seed bundles + `.rcbundle` archive import, with LAN sync
as a follow-up**. The reasoning, since it constrains several sub-tasks:

A read-only library baked into the APK does not survive contact with the data.
`library/perf3-final/` is **146 MB** — a 73 MB `background.mp4` plus a 79 MB
uncompressed `backing.wav`. One movie-backed song exceeds the Play Store base
budget on its own. And a fixed library removes the point of the app: the value is
that you learn *the song you chose*.

On-device import is not available at any price — the pipeline is Python plus
yt-dlp plus ffmpeg subprocesses.

So the phone is a **consumer of bundles, not a producer of them**:

1. **Seed** the three MIDI-only bundles into the APK (4 KB each) so first launch
   is not an empty list. Cheap, and not the real answer.
2. **`.rcbundle` archive import** (M15-B) — a zipped bundle directory, opened via
   Android's document picker and share-sheet intent, so no broad storage
   permission is needed and transfer works over USB, Drive, email, anything.
   This is an `ExportBundle` / `ImportBundle` **`HostCommand` pair**, never a
   mobile-only IPC call (CLAUDE.md: *"a new user-facing capability gets an
   `Action` (if pure) or a `HostCommand` (if it does I/O) — never a one-off IPC
   command"*). Desktop-to-desktop song sharing falls out of it for free.
   Export **transcodes for mobile** as it packs: that 79 MB wav becomes ~5 MB of
   Opus and a phone-resolution movie re-encode brings the 146 MB bundle under
   20 MB.
3. **LAN pull from desktop** (M15-G, follow-up) — the desktop app already runs a
   WebSocket control server. Teach it to list and serve bundles; the phone pulls
   directly. This matches the actual workflow: import on the desktop where the
   sidecar lives, practise away from it.

## Task breakdown

Ordered. M15-0 gates everything; M15-A and M15-B are desktop-side and
sandbox-safe; M15-C onward need the hardware.

| Id | Task | Area | Tier | Hardware |
|---|---|---|---|---|
| **M15-0** | WebView performance spike — decides D1 | infra | opus | Android device |
| **M15-A** | Extract `PlaySession` → `crates/play` | core | sonnet | none |
| **M15-B** | `.rcbundle` export/import `HostCommand` pair | core/infra | sonnet | none |
| **M15-C** | Mobile crate + Tauri Android scaffold, shared frontend package, storage wiring | infra | opus | Android device |
| **M15-D** | Kotlin `android.media.midi` plugin + `NoteSource` impl | midi | opus | piano + device |
| **M15-E** | Android audio: oboe output, compact sf2 asset, latency calibration | audio | opus | piano + device |
| **M15-F** | Phone-landscape highway layout + touch-piano `NoteSource` | tauri | sonnet | device |
| **M15-G** | LAN bundle sync from desktop control server | infra | sonnet | device |

**Routing note:** `loc:local` currently means "needs the physical piano". Most of
M15 needs *an Android device*, which is a different constraint — M15-0, M15-C,
M15-F and M15-G need a phone but no piano. Add a `loc:android` label rather than
overloading `loc:local`, so the cloud queue keeps working and a phone-only task
is not blocked behind piano availability.

## Risks

1. **Canvas 60 fps in Android WebView, over a playing `<video>`.** The single
   most likely thing to sink D1. `HighwayScreen.tsx` scrubs a `<video>` backdrop
   by assigning `currentTime` every frame; that is cheap on desktop WebView2 and
   may not be on a mid-range Android. This is what M15-0 measures.
2. **MIDI timestamp fidelity across the plugin bridge.**
   `MidiReceiver.onSend(msg, offset, count, timestamp)` provides a `nanoTime`
   timestamp. It must be carried through to `NoteEvent::timestamp_us`, *not*
   re-stamped on arrival in Rust. The CLAUDE.md invariant that scoring keys off
   MIDI timestamps rather than the render loop lives or dies here, and a plugin
   channel is exactly where such a thing gets silently dropped.
3. **`cpal`/oboe maturity.** Least-tested backend in the dependency tree.
   Fallback if it misbehaves: render the synth to a buffer in Rust and hand PCM
   to an `AudioTrack` on the Kotlin side, which is the same shape as the MIDI
   plugin and therefore not new risk.
4. **USB device permission flow.** Android prompts per-device on attach. An
   `intent-filter` with a `usb_device_filter` resource can auto-grant on connect
   and launch the app; without it every session starts with a dialog.
5. **Shared-code drift.** The consequence of D2. Mitigated by M15-A and the
   frontend package in M15-C — both of which must land *before* the mobile shell
   grows any play logic of its own.

## Non-goals

- Composing, editing, recording, or importing on the device.
- iOS.
- Play Store distribution. `tauri android build --apk` + `adb install` is the
  distribution story for now.
- Changing anything about the desktop app's behaviour. M15-A and M15-B refactor
  and extend it; neither may alter what it does.

## Testing on device

Keep the control socket in the mobile build. With
`adb forward tcp:9001 tcp:9001` and `ROCKCRAFT_CONTROL_ADDR`, the existing
`rockcraft-control` examples and the `query help` catalog drive the phone from a
dev machine unchanged — the same agent-control surface documented in
`docs/AGENT-CONTROL.md`, no mobile-specific test harness needed.

CI keeps the current gate (`fmt · clippy · test`) as the merge condition. Add a
non-blocking `cargo check --target aarch64-linux-android` job once M15-C lands;
the NDK download is too slow to sit in the blocking path, and M15-A means the
play logic is covered by host-side tests anyway.
