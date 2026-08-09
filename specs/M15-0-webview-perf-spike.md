# M15-0 — Spike: can an Android WebView hold the highway's frame rate?

> Milestone: M15 — Android · Issue: #<n> · Suggested tier: opus
> Branch: `claude/m15-0-webview-spike`

## Goal

Decide, with measurements from a real device, whether Tauri v2 + Android WebView
can render the existing note highway at playable frame rates — **including** over
a background movie. This is the go/no-go on decision D1 in
[`M15-android-overview.md`](M15-android-overview.md); no other M15 task starts
until it reports.

The deliverable is a **written report and a recommendation**, not shippable code.

## Context

- The renderer under test already exists: `tauri-app/src/screens/highway/`
  (`HighwayCanvas.ts`, 829 LOC of canvas2d; `HighwayScreen.tsx`, 673 LOC of
  orchestration). Nothing about it needs to change for the spike.
- `tauri-app/src-tauri/Cargo.toml` already builds a `cdylib`, so
  `tauri android init` has something to link against without a new crate.
- **The prime suspect** is in `HighwayScreen.tsx` around lines 276–290: the
  backdrop is a paused, muted `<video>` whose `currentTime` is *assigned every
  frame* to scrub it to song position. On desktop WebView2 that is cheap. On
  Android it forces a seek-and-decode per frame and is the likeliest cause of a
  failure, so the spike must measure the alternative (below) before concluding
  anything about the WebView itself.
- `library/perf3-final/` is the realistic worst case on hand: a movie backdrop
  plus a real chart. `library/over-the-rainbow-piano/` is the MIDI-only case.

## What to do

### 0-a — Minimal Android scaffold (throwaway)

`tauri android init` against the existing `tauri-app`, enough to build a debug
APK and load the current frontend. Expect to need:

- `assetProtocol` reachable from the Android WebView (`tauri.conf.json` already
  enables it with scope `**`) — confirm `convertFileSrc` resolves on device, as
  the backdrop `<video>` depends on it.
- A bundle readable on device: push one to app-private storage with `adb push`
  and point `ROCKCRAFT_LIBRARY_DIR` at it. Hardcoding the path in the spike is
  fine.

Everything failing to compile because MIDI or audio is unavailable on Android is
**expected** — stub or `#[cfg]`-out whatever blocks the build. The spike measures
rendering only.

### 0-b — Instrument frame times

Add a temporary overlay (or console reporter) capturing, from
`requestAnimationFrame` deltas over a 60-second run:

- p50 / p95 / p99 frame time, in ms
- count of frames over 33 ms (a visible hitch)
- the same numbers for the canvas draw call alone (`performance.now()` around
  the `HighwayCanvas` render), so WebView compositing cost is separable from
  our drawing cost

### 0-c — The matrix

Run each cell for 60 s on a **mid-range phone in landscape** (not a flagship —
the flagship will pass and tell us nothing):

| # | Backdrop | Chart | Notes |
|---|---|---|---|
| 1 | none | MIDI-only bundle | floor: our drawing cost alone |
| 2 | none | synthetic dense chart | worst-case note count on screen |
| 3 | `<video>`, per-frame `currentTime` scrub | `perf3-final` | reproduces desktop behaviour exactly |
| 4 | `<video>`, native `play()` + drift correction | `perf3-final` | the proposed fix — see below |
| 5 | one background image layer, keyframed | any | M14-D path, cheap but unmeasured on device |

**Synthetic dense chart** for #2: sixteenth notes in both hands at 120 bpm,
sustained for 30 s (≈240 notes on the highway at once given `LEAD_US` of 2 s).
Generate it, don't hand-author it.

**Drift correction** for #4: call `play()` on the `<video>` once and let it run on
its own clock, assigning `currentTime` *only* when
`|videoTime − expectedTime| > 100 ms`. Same alignment maths as today
(`videoTime = (songTime − shift) + offset`), just not re-seeking every frame.
If #4 passes where #3 fails, the WebView is fine and the fix is a frontend
change — that is the most likely outcome and the most valuable thing this spike
can establish.

### 0-d — Also check, cheaply

- **Device pixel ratio.** A 1080×2400 phone at DPR 3 gives a backing store far
  larger than the highway needs. Record whether capping the canvas at DPR ≤ 2
  changes the numbers; if it does, that is a free win for M15-F.
- **Thermal behaviour.** Re-run the worst passing cell after 10 minutes of play.
  A phone that passes cold and throttles to 20 fps warm has not passed.
- **Audio/MIDI are out of scope here.** Do not attempt them.

## Report

Commit `docs/spikes/M15-0-webview-perf.md` containing:

- Device model, Android version, WebView version, whether it thermally throttled
- The full matrix with p50/p95/p99 and hitch counts
- A recommendation, exactly one of:
  - **GO** — proceed with M15-A onward as specced
  - **GO, with the backdrop change** — proceed, and M15-F additionally owns
    switching the backdrop to native playback + drift correction (note whether
    desktop should adopt it too)
  - **NO-GO** — with the numbers that condemn it, plus which alternative the
    evidence favours (native Kotlin frontend over a shared Rust core via
    UniFFI/JNI, or WebGL instead of canvas2d)

State the pass bar up front and hold to it: **p95 ≤ 20 ms with a backdrop, no
thermal collapse.** A cell that only clears 30 fps is a conditional pass and must
say so rather than being rounded up.

## Scope boundaries (do NOT)

- Do **not** add a workspace member, and do not create `mobile-app/` — that is
  M15-C's job.
- Do **not** modify anything under `tauri-app/src/screens/highway/` in a way
  intended to survive. Instrumentation and the #4 backdrop variant are throwaway;
  if #4 wins, M15-F implements it properly.
- Do **not** commit `gen/android/`, the debug APK, or NDK/SDK paths.
- Do **not** touch `crates/core`, `crates/midi`, or `crates/audio`.
- Do not add third-party dependencies. Frame timing needs
  `performance.now()`, nothing else.

## Acceptance

- [ ] `docs/spikes/M15-0-webview-perf.md` committed, with the full matrix and one
      of the three verdicts
- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green — i.e. the spike left `main`'s behaviour and
      the desktop build untouched
- [ ] PR opened against `main` from the branch above, `Closes #<n>`
