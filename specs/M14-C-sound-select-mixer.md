# M14-C — audio: player instrument selection + separate volume mixer

> Milestone: M14 — Play-screen polish · Issue: #259 · Suggested tier: sonnet
> Branch: `claude/issue-259-difukx`

## Goal

Let the player pick what their own notes sound like, and set the level of the
three things playing at once — **you**, **the song**, and **the backing track**
— independently of each other.

## Context

Playing along produces three simultaneous sources and, before this task, one
knob for none of them:

- The live piano's notes, echoed by the synth (`SynthHandle::apply`).
- The chart auditioned by "hear the song" (M2-B / M13-C).
- The decoded backing file (`BackingHandle`).

The synth rendered everything on **one** MIDI channel (`synth.rs`'s old
`MIDI_CHANNEL: i32 = 0`), so the two synth sources were physically the same
voice: one timbre, one level. The only instrument control was the
`ROCKCRAFT_SYNTH_PROGRAM` env var, which is a launch-time debugging aid, not a
user-facing choice.

Split the voice in two and give each source a fader.

## What to do

**`core` (pure settings — no audio).** New `crates/core/src/mixer.rs`:

```rust
pub enum SynthBus { Player, Song }   // midi_channel() -> 0 / 1
pub enum MixerBus { Player, Song, Backing }  // synth_bus() -> Option<SynthBus>
pub struct Gain(f32);                // new() clamps 0.0..=1.0, None if non-finite
pub struct Instrument { id, name, program }  // curated GM list, ~15 entries
pub struct Mixer { player: BusMix, song: BusMix, backing_gain: Gain }
pub struct MixerReport { .. }        // Mixer + the catalog, one round trip
```

The catalog is the single source of truth for the selectable sounds: no
frontend hardcodes its own list.

**`audio` (applies the settings).** `SynthHandle` carries a `SynthBus` and
addresses that bus's MIDI channel; `for_bus` hands back a sibling on the other
bus over the same command queue. New per-bus `set_instrument` (program change)
and `set_gain` (controller 7, channel volume); `all_off` stays bus-wide (it is
the panic button). `BackingHandle::set_gain` sets the sink volume.

**`control` (the protocol seam).** Three `HostCommand`s — I/O, so they cannot be
`core::Action`s: `set_instrument { bus: SynthBus, instrument: String }`,
`set_bus_gain { bus: MixerBus, gain: f32 }`, `query_mixer`. Each returns the new
`MixerReport`. `SynthBus` in the signature is what makes "an instrument for the
backing track" unrepresentable rather than a runtime error.

**Tauri.** `AudioState` owns the `Mixer` and pushes each change at the synth /
backing thread; the backing level is sticky across sink restarts exactly like
the playback speed. Play-mode notes route to their buses in `tick_play`: the
player's live MIDI on `Player` (new — the desktop app did not echo them at all,
unlike the TUI), the hear-song auditions on `Song`. Three Tauri commands mirror
the host commands. The webview gets a `MixerPanel` over the highway: an
instrument dropdown per voice and three faders, persisted to `localStorage` and
re-applied on mount (the backend's mix is per-run).

**TUI.** The play screen sounds the song on `Song` and the player on `Player`;
the shell holds the `Mixer` and wires the three host commands, carrying the
backing level onto each new take.

## Tests

- `core`: gain clamps / rejects non-finite; buses map to distinct channels;
  catalog ids and programs unique; setting one bus leaves the others alone; the
  report serialises the mix *and* the catalog.
- `audio`: a handle starts on `Player`; `for_bus` tags commands with the other
  channel over the same queue; instrument/gain address the handle's own bus;
  `all_off` is bus-wide.
- `control`: the new commands round-trip and appear in `host_help`; a `backing`
  bus is rejected for `set_instrument` at parse time.
- Tauri: mixer settings apply with **no** audio device (the headless CI path);
  faders are independent; a bad instrument id / non-finite gain is reported and
  changes nothing.
- TUI: mixer commands work off the play screen; bad input is a failed command;
  a backing level set between takes reaches the next play screen.
- Frontend (vitest): stored prefs parse tolerantly (absent / malformed / wrong
  types / out-of-range) and an instrument the catalog no longer offers is
  dropped while the gains survive.

## Scope boundaries (do NOT)

- Do not add per-bundle or per-song mix persistence — this is a machine-local
  preference, not bundle metadata.
- Do not add audio effects (EQ, reverb, pan) or a master fader.
- Do not commit a SoundFont. Instrument selection is only audible with a full
  GM bank; document that rather than shipping one.
- Do not change scoring, the wait gate, or the highway rendering.

## Acceptance

- [x] `cargo fmt --all --check` clean
- [x] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [x] `cargo test --workspace` green
- [x] `npx tsc --noEmit` + `npm test` clean in `tauri-app`
- [ ] Manual (local): with a GM SoundFont, the player's notes change timbre from
      the dropdown and each fader moves only its own source
- [x] PR against `main` from `claude/issue-259-difukx`, `Closes #259`
