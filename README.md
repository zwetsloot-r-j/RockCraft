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
cargo run -p rockcraft-tui -- --mock           # no piano: play with the number row (1-0)
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
| Record | play notes (mock: number row `1`-`0` = C-major) · `s` save take · `Tab`/`Esc` back to menu |
| Play | `r` restart · `m` toggle hear song · `Tab`/`Esc` back to menu |
| Edit | letters/symbols = editor commands · `R` arm record, then number row `1`-`0` plays notes · `?` help |

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

## Importing from a video (Synthesia tutorials)

Turn a Synthesia-style piano-tutorial video into a playable chart, from the
menu (**Import from video file…**, or **Import from URL…**). The importer ships
in-repo; the *downloader*, the videos, and the extracted charts do **not** —
see the content policy in [`docs/IMPORT.md`](docs/IMPORT.md). Imported charts
land in the gitignored `import-out/`.

One-time setup:

1. **Extractor sidecar — required for any import.** Install its Python deps and
   `ffmpeg`:

   ```sh
   pip install -r tools/synthesia-extract/requirements.txt   # numpy, opencv-headless
   ```

   ⚠️ The pipeline runs the sidecar as **`python3 tools/synthesia-extract/extract.py`**
   (bare `python3`). Install the deps into the `python3` on your `PATH`, **or**
   activate a virtualenv before launching the app — a venv that isn't active
   won't be seen. (Details: [`tools/synthesia-extract/README.md`](tools/synthesia-extract/README.md).)

2. **URL downloads — optional, only for "Import from URL".** Downloading is not
   committed; provide a private fetch hook and a downloader:

   - Put an executable script at `scripts/local/fetch.sh` (gitignored, not
     shipped — copy it between machines yourself), or point `ROCKCRAFT_FETCH_CMD`
     at one. It is invoked as `fetch.sh <URL> <TARGET_PATH>` and must leave the
     downloaded video at exactly `<TARGET_PATH>` (exit non-zero on failure).
   - Install a downloader the hook calls — e.g. `pipx install yt-dlp` so it is on
     your `PATH`. Without a configured hook, only **Import from video file…** is
     offered.

## Agent control interface (drive it programmatically)

A running RockCraft can expose a **localhost-only WebSocket** so an AI agent (or
a script) can edit the composer using the same actions the keyboard triggers.

```sh
cargo run -p rockcraft-tui -- --control          # bound addr printed to stderr:
                                                 #   Control server listening on ws://127.0.0.1:<PORT>
ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9001 cargo run -p rockcraft-tui   # pin a stable address (also enables it)
```

Connect to `ws://127.0.0.1:<PORT>`. The server sends a `hello` banner on
connect; send `{"type":"query","what":"Help"}` to list every action with its
parameters, then `run_action` and read back the state snapshot.

- Protocol reference: [`docs/AGENT-CONTROL.md`](docs/AGENT-CONTROL.md)
- Guided demo (every action, with the equivalent TUI key): [`docs/DEMO-SCENARIO.md`](docs/DEMO-SCENARIO.md)
- Minimal client: [`crates/control/examples/agent_session.rs`](crates/control/examples/agent_session.rs)

## Notes

- Anything needing the physical piano (live MIDI capture, latency, audio out) is
  local-only and can't run in a cloud sandbox. Cloud agents work on `core`, file
  parsing, and scoring against committed MIDI fixtures.
