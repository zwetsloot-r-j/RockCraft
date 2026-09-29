# M18-A — TUI mixer: editable and remembered

> Milestone: M18 · Issue: #296 · Suggested tier: sonnet
> Branch: `claude/tui-mixer`

## Goal

Let a TUI user set the volume of their own notes, the song and the backing
(and the two instruments) from the keyboard, and keep those settings between
runs. Today the TUI can only change the mix over the control socket, and it
resets to the defaults on every start.

## Context

- The mix is `core::Mixer` (`crates/core/src/mixer.rs`): a `BusMix`
  (instrument + `Gain`) for `player` and `song`, plus `backing_gain`.
  `Mixer::set_gain` / `set_instrument` validate; `instruments()` is the catalog.
- The TUI shell owns one `Mixer` and pushes changes at the audio through
  `Shell::apply_mixer` (`crates/tui/src/app.rs`). The `set_bus_gain`,
  `set_instrument` and `query_mixer` host commands already go through it.
- The desktop app's equivalent is `MixerPanel.tsx`, which remembers the mix in
  `localStorage` (`mixerPrefs.ts`). This spec is the TUI's counterpart; the two
  don't share storage.
- Related: `specs/M14-C-sound-select-mixer.md` (the mixer).

## Behaviour

**The mixer overlay.** `x` opens a small mixer box over the play screen and the
edit screen; `x` or `Esc` closes it. While it is open it takes the keys:

| Key | Effect |
|-----|--------|
| `↑` / `↓` | Select a row: You level, Song level, Backing level, You instrument, Song instrument |
| `←` / `→` | Level rows: −/+ 0.05 (clamped 0.0–1.0). Instrument rows: previous / next in the catalog |
| `Home` / `End` | Level rows: jump to 0.0 / 1.0 |

Each row shows its value as a bar and a number (`You   ████████░░ 0.80`).
Changes apply at once, through `Shell::apply_mixer`, so they sound immediately
and the socket's `query_mixer` sees them. Keys that the overlay does not use
are ignored while it is open (they don't leak to the screen underneath).

The status line of the play and edit screens shows `[x] mix` among its hints.

**Remembering.** The mix is saved to a settings file after every change (from
the overlay or from `set_bus_gain` / `set_instrument` over the socket), and
loaded and applied when the TUI starts.

- Location: `$ROCKCRAFT_CONFIG_DIR/tui-settings.json` when that variable is set
  (tests use it); otherwise `%APPDATA%\RockCraft\tui-settings.json` on Windows,
  `$XDG_CONFIG_HOME/rockcraft/tui-settings.json`, else
  `~/.config/rockcraft/tui-settings.json`. Create the directory if needed. No
  new dependencies: plain `std::env` + `std::fs`.
- Format:
  ```json
  { "version": 1,
    "mixer": { "player": { "instrument": "grand_piano", "gain": 1.0 },
               "song":   { "instrument": "grand_piano", "gain": 0.8 },
               "backing_gain": 1.0 } }
  ```
- A missing file means defaults. A file that doesn't parse, or has an unknown
  instrument id or an out-of-range gain, never stops the TUI from starting: use
  the default for each bad field and keep the good ones, and print one warning
  to stderr before the terminal UI starts.
- A failed save (read-only disk etc.) shows once in the status bar and is not
  retried every frame.
- Writes happen on the app thread after a change, never on the MIDI or audio
  thread; the file is written to a temp name then renamed so a crash can't
  leave half a file.

## What to do

- `crates/tui/src/settings.rs` (new): `TuiSettings { mixer: Mixer }`,
  `settings_path() -> Option<PathBuf>`, `load(path) -> (TuiSettings,
  Vec<String> /* warnings */)`, `save(path, &TuiSettings) -> io::Result<()>`.
  Parse with `serde_json::Value` field by field (so one bad field doesn't lose
  the rest) and apply through `Mixer`'s validating setters.
- `main.rs`: load before building the shell; hand the mixer to `Shell::new`,
  which applies it to the synth at start (the same push `apply_mixer` does).
- `app.rs`: the overlay state and keys; call save after each successful mixer
  change.
- `play.rs` / `edit.rs`: draw the overlay (a helper shared by both screens).
- `docs/AGENT-CONTROL.md`: note that the TUI now remembers mixer changes made
  over the socket.

## Tests

- `settings`: round trip; missing file → defaults; bad JSON → defaults + a
  warning; one bad field (unknown instrument, gain 3.0, gain "loud") → that
  field defaults, the others load; `settings_path` honours
  `ROCKCRAFT_CONFIG_DIR`.
- Overlay (headless shell): `x` opens; `↓` `→` raises Song by 0.05; `←` at
  0.0 stays 0.0; `End` sets 1.0; instrument rows cycle and wrap; `Esc` closes; a loop key like
  `[` does nothing while open.
- A mixer change (overlay or `set_bus_gain`) writes the file; a new shell built
  from that file starts with the saved mix.
- Rendering: the overlay fits an 80×24 terminal.

## Scope boundaries (do NOT)

- Don't change the desktop app or its `localStorage` prefs.
- Don't persist anything but the mixer (other settings may join the file later;
  keep the `version` field for that).
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #296`
