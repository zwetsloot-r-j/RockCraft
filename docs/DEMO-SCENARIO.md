# Demo Scenario — the full action vocabulary, agent ⇄ human

> **Status:** Stable (M4). Companion to [`AGENT-CONTROL.md`](AGENT-CONTROL.md).

This is a single guided session that exercises **every** composer action once.
It exists so that:

- **an AI agent can present it** — drive the beats over the control protocol
  while narrating each step;
- **a human can repeat it** — every beat lists the equivalent TUI keystroke, so
  you can follow along on the keyboard and build muscle memory;
- **CI can verify it** — the executable twin
  [`crates/control/tests/demo_scenario.rs`](../crates/control/tests/demo_scenario.rs)
  runs the agent side beat-for-beat and asserts each state snapshot. The doc and
  the test share beat numbers; keep them in sync.

The point of the parity columns is to prove **the agent and the human drive the
exact same engine** — an `run_action` over the wire and a keypress in the TUI map
to the identical `core::Action`.

## How to run it

**As an agent (programmatic):**

```bash
# Terminal 1 — start the TUI with the control server
cargo run --bin rockcraft-tui -- --control      # logs: Control server bound to 127.0.0.1:<PORT>
```

Then connect a WebSocket client to `ws://127.0.0.1:<PORT>` and send the
`run_action` / `query` frames below in order (see `AGENT-CONTROL.md` for the
message shapes and the `agent_session.rs` example for a working client).

**As a human (keyboard):** open the composer/edit screen in the TUI and press the
keys in the **Human key** column, in order. Press `?` at any time for the in-app
help overlay (the human counterpart to `query { what: "Help" }`).

**As a test:**

```bash
cargo test -p rockcraft-control --test demo_scenario
```

## Notation

- **Agent** column: the `action` name + JSON `params` for a `run_action` request,
  e.g. `set_cursor {pitch:60, step:0}`. Nullary actions take `{}`.
- **Human key** column: the TUI keystroke (from `crates/tui/src/edit.rs`).
  `—` means there is no single-key binding (the action is reached by navigation
  or is agent-only); the note says how a human achieves the same thing.
- Grid defaults: **120 BPM, 4/4, sixteenth subdivision** → one step = 125 000 µs,
  one bar = 16 steps. `add_note` places a note of duration 1 step, velocity 80.

---

## Act 0 — Discovery

Before driving anything, learn the vocabulary live.

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 0 | *(read the connect banner — sent unsolicited)* | — | `hello` frame naming the verbs + query kinds, hinting at `query Help` |
| 0a | `query {what:"Help"}` | `?` (help overlay) | Returns every action with its `params` schema + description. Covers exactly `action_names()`. |
| 0b | `query {what:"Actions"}` | — | The name-only list; a subset of `help`. |

> The banner is the in-band counterpart to this doc: an agent that connects
> "cold" learns from it that `query Help` exists. A correct client skips
> unsolicited `hello`/`event` frames when correlating responses.

> Casing note: `what` uses the Rust variant names — `State`, `Actions`, `Help`,
> `Render` (PascalCase).

## Act 1 — Grab & delete a scratch note (nets zero notes)

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 1 | `set_cursor {pitch:50, step:0}` | navigate `h/j/k/l` | cursor at (50, 0) |
| 2 | `add_note {}` | `a` (or `i`) | 1 note at pitch 50 |
| 3 | `toggle_grab {}` then `cursor_right {}` | `m` then `l` | the grabbed note slides to step 1 (start_us = 125 000) |
| — | `toggle_grab {}` | `m` | drop it |
| 4 | `delete_note {}` | `x` (or `d`) | scratch note removed → 0 notes |

## Act 2 — Build & shape a motif

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 5 | `set_cursor {pitch:60, step:0}` → `add_note {}` | navigate, then `a` | root C4 at step 0: dur 125 000, velocity 80 |
| 6 | `resize_note {delta_steps:1}` | `]` (`[` shortens) | C4 duration → 2 steps (250 000) |
| 7 | `adjust_velocity {delta:8}` | `+` / `=` (`-` lowers) | C4 velocity → 88 |

## Act 3 — Navigate every which way

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 8 | `cursor_right {}` / `cursor_left {}` | `l` / `h` | step +1 then back to 0 |
| 9 | `cursor_up` / `cursor_down` / `cursor_octave_up` / `cursor_octave_down` | `k` / `j` / `K` / `J` | pitch 60→61→60→72→60 |
| 10 | `cursor_bar_right {}` / `cursor_bar_left {}` | `L` / `H` | step +16 (one bar) then back |
| 11 | `cursor_to_end {}` / `cursor_to_start {}` | `$` / `0` | jump to end of content, then step 0 |
| 12 | `subdivision_finer {}` / `subdivision_coarser {}` | `>` / `<` | subdivision Sixteenth→ThirtySecond→Sixteenth |

## Act 4 — A second note, then chords

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 13 | `set_cursor {pitch:64, step:4}` → `add_note {}` | navigate, then `a` | 2 notes (adds E4) |
| 14 | `set_cursor {pitch:67, step:8}` → `enter_chord_mode {}` | navigate, then `c` | triad preview appears = 3 ghost notes |
| 15 | `cycle_chord_degree {delta:1}` / `set_chord_degree {degree:1}` | `]`/`[` / digits `1`–`7` (chord mode) | preview re-voices, still 3 notes |
| 16 | `toggle_chord_kind {}` | `s` (chord mode) | triad → seventh = 4 notes |
| 17 | `commit_chord {}` | `Enter` (chord mode) | preview becomes permanent; selector closes |
| 18 | `enter_chord_mode {}` then `cancel_chord {}` | `c` then `Esc` | preview appears then rolls back cleanly (no leftover notes) |

## Act 5 — Input mode (record arm / flavour)

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 19 | `toggle_record_arm` → `toggle_record_flavour` → `toggle_record_arm` | `R` → `t` → `R` | input_mode DirectEdit→StepRecord→LiveRecord→DirectEdit |

## Act 6 — Transport (pure: time is injected, never wall-clock)

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 20 | `toggle_play_cursor {}` ×2 | `Space` ×2 | playing true, then false |
| 21 | `play_from_start {}` | `P` | playing; playhead_us = 0 |
| 22 | `play {from_us:500000}` | — (agent-only) | playhead_us = 500 000 |
| 23 | `stop {}` | — (`Space` toggles while playing) | playing false |
| 24 | `set_playhead {us:1000000}` | — (agent-only) | moves the *record* playhead (used by live-record) |

## Act 7 — Loop, metronome, count-in

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 25 | `toggle_metronome {}` | `M` | metronome on |
| 26 | `set_loop_bounds {start_us:0, end_us:1000000}` then `toggle_loop {}` | — , then `o` | loop region set; looping on |
| 27 | `start_count_in_record {}` | `C` | input_mode LiveRecord and playing (count-in running) |

## Act 8 — Selection & clipboard

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 28 | `set_cursor {60,0}` → `start_selection {}` → `set_cursor {84,16}` | navigate, `v`, navigate | selection rectangle pitch 60–84, from step 0 |
| 29 | `yank_selection {}` | `y` | clipboard filled (motif + chord); selection clears; timeline unchanged |
| 30 | `set_cursor {48,32}` → `paste_clipboard {}` | navigate, `p` | notes grow by clipboard length |
| 31 | `start_selection {}` → move → `clear_selection {}` | `v` → move → `Esc` | selection cancelled |
| 32 | select wide → `delete_selection {}` | `v`, move, `D` | selected notes deleted |

## Act 9 — History

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 33 | `undo {}` then `redo {}` | `u` then `U` | undo restores the deleted notes; redo removes them again |

## Act 10 — Protocol features (not actions)

These are transport-level queries/subscriptions, included so a presenter shows
the whole surface, not just `run_action`.

| Beat | Agent | Human key | Expected |
|------|-------|-----------|----------|
| 34 | `query {what:"State"}` | — | snapshot matching the last `run_action` |
| 35 | `query {what:"Render"}` | (the live screen) | the text-screenshot channel |
| 36 | `subscribe {topic:"Events"}` / `unsubscribe {topic:"Events"}` | — | `ok` both times |

---

## Keeping doc and test in sync

The beat numbers above are the section markers in
`crates/control/tests/demo_scenario.rs`. When you add or rename an action:

1. add it to `core::Action`, `action_names()` and `action_help()` (parity tests
   in `crates/core/src/action.rs` enforce all three agree);
2. add a beat here and the matching assertion in the test;
3. if it has a TUI binding, wire it in `crates/tui/src/edit.rs` and fill the
   **Human key** column.

## See also

- [`AGENT-CONTROL.md`](AGENT-CONTROL.md) — the protocol and discovery queries
- [`crates/control/tests/demo_scenario.rs`](../crates/control/tests/demo_scenario.rs) — the executable twin
- [`crates/control/examples/agent_session.rs`](../crates/control/examples/agent_session.rs) — a minimal client
