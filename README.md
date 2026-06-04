# RockCraft

Learn songs on a USB-MIDI digital piano: a scrolling Synthesia-style note
highway plus Rocksmith-style scoring. Input is clean USB-MIDI (note-on/off,
pitch, velocity, timing), so the core loop is precise and deterministic.

Cargo workspace, crates depend inward only:
`core` (pure domain) ← `midi` (live input + file I/O) ← `audio` (synth) ←
`tui` (ratatui frontend). See `CLAUDE.md` for the full architecture contract.

## Build & test

```sh
cargo build --workspace                       # build everything
cargo test  --workspace                       # run all tests
cargo fmt   --all --check                     # formatting gate
cargo clippy --workspace --all-targets        # lint gate (warnings = errors)
```

The three gate commands (`fmt --check`, `clippy`, `test`) are the CI merge gate
in `.github/workflows/ci.yml` — run them before opening a PR.

## Run the app (TUI)

```sh
cargo run -p rockcraft-tui                     # connect to piano (port "casio")
cargo run -p rockcraft-tui -- <port-substr>    # match a different MIDI port name
cargo run -p rockcraft-tui -- --mock           # no piano: play with the QWERTY keys
cargo run -p rockcraft-tui -- <port> <backing.wav>  # play a backing track while recording
```

Positional args: `[port-name-substring] [backing-audio-file]` (wav/mp3/ogg/flac).
`--mock` forces the keyboard mock; with no matching port the app also falls back
to the mock so it always launches. Audio is optional — if no SoundFont/output
device is available it runs silently.

### Controls

| Screen | Keys |
| --- | --- |
| Menu | `↑`/`k` `↓`/`j` move · `Enter` select · `q`/`Esc` quit |
| Record | type/play notes · `s` save take · `Tab`/`Esc` back to menu |
| Play | `r` restart · `m` toggle hear song · `Tab`/`Esc` back to menu |

Menu items: **Record**, **Play last recording**, **Quit**. Recordings are saved
as `recordings/take-*/song.mid`; "Play last recording" loads the most recent.

## Echo example (raw MIDI diagnostics)

Connect to the piano and print live note events with timing/latency columns:

```sh
cargo run -p rockcraft-midi --example echo            # port "casio"
cargo run -p rockcraft-midi --example echo -- <substr>  # match another port
```

## Audio / SoundFont

The synth loads `crates/audio/assets/piano.sf2` by default. Override the path
with the `ROCKCRAFT_SF2` environment variable:

```sh
ROCKCRAFT_SF2=/path/to/piano.sf2 cargo run -p rockcraft-tui
```

## Notes

- Anything needing the physical piano (live MIDI capture, latency, audio out) is
  local-only and can't run in a cloud sandbox. Cloud agents work on `core`, file
  parsing, and scoring against committed MIDI fixtures.
