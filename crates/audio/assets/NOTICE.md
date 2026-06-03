# SoundFont asset

The piano synth ([`crates/audio`]) loads a SoundFont (`.sf2`) at runtime — it is
**not** committed to the repo (SoundFonts are multi-megabyte binaries with their
own licenses, and which piano you want is a taste/quality choice).

## Where it goes

Drop a SoundFont here:

```
crates/audio/assets/piano.sf2
```

…or point `ROCKCRAFT_SF2` at one anywhere:

```sh
ROCKCRAFT_SF2=/path/to/your/piano.sf2 cargo run -p rockcraft-tui
```

If no SoundFont is found the app still runs — it just starts without audio and
prints a one-line warning.

## Picking one

For this app (whose whole point is piano feel) prefer a **piano-only** SoundFont
over a full General MIDI set: a focused piano font spends its entire sample
budget on the piano, so it usually sounds *better* than the piano buried in a
140 MB GM font, while being a fraction of the size. When choosing, look for:

- **Multiple velocity layers** — different samples for soft vs. hard strikes;
  this is what gives the keyboard dynamics.
- A **redistributable license** if you intend to commit/ship it. Many freely
  available piano SoundFonts permit redistribution; confirm the specific font's
  license and record its source/author/license alongside the file before
  committing it.

Target size: a few MB is plenty for an MVP.
