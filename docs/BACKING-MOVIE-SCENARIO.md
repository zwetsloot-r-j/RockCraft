# Backing-Movie Scenario — author with a movie, save, play, quit (agent ⇄ human)

> **Status:** Stable. Companion to [`AGENT-CONTROL.md`](AGENT-CONTROL.md); a
> task-shaped sibling to the vocabulary tour in [`DEMO-SCENARIO.md`](DEMO-SCENARIO.md).

This is a single end-to-end session an **AI agent drives over the control
socket** against the **Tauri desktop app**: start the game, edit a brand-new song
with a *movie as backing*, add a few notes, save it, load it back in **playback
mode**, confirm the score loop runs, verify the song was persisted, then **shut
the app down** — autonomously, start to finish.

It exists so that:

- **an agent can run it** — every beat is a `run_action` / `run_host_command` /
  `query` frame; the executable twin
  [`crates/control/examples/backing_movie_session.rs`](../crates/control/examples/backing_movie_session.rs)
  performs exactly these beats and asserts each result;
- **a human can repeat it** — every beat lists the equivalent desktop edit-screen
  action, proving the agent and the human drive the **same** backend.

Unlike `DEMO-SCENARIO.md` (a pure-composer tour over `run_action`), this scenario
leans on the **host-command tier** (`run_host_command`): attaching the movie,
saving, loading to play, and quitting are app-level I/O, so they are
`control::HostCommand`s, not `core::Action`s.

## How to run it

**As an agent (one command):**

```bash
cargo build --bin rockcraft-tauri          # build the app once
cargo run -p rockcraft-control --example backing_movie_session
```

The driver launches the app itself (pinning `ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9001`,
so it knows the socket address without parsing stderr), drives the beats below,
and ends by telling the app to quit. On a headless host it launches under
`xvfb-run` when available. It prints each beat and finishes with `SCENARIO OK`.

**As an agent (by hand):** start the app with the control socket, then send the
frames yourself:

```bash
ROCKCRAFT_CONTROL_ADDR=127.0.0.1:9001 cargo run --bin rockcraft-tauri -- --control
# then connect a WebSocket client to ws://127.0.0.1:9001 (see AGENT-CONTROL.md)
```

**As a human (desktop):** open the edit screen and follow the **Human action**
column.

> **Running the GUI on a Windows host from a WSL checkout?** See the runbook
> [`RUN-ON-WINDOWS-HOST.md`](RUN-ON-WINDOWS-HOST.md) — it covers the
> `--features tauri/custom-protocol` build flag (or the webview shows "localhost
> connection refused"), driving the socket from WSL past the proxy/sandbox, and
> `scripts/drive-backing-movie.mjs` (a WSL-side driver that attaches to an
> already-running app instead of spawning one).

## Notation

- **Agent** column: the request — `action`/`command` name + JSON `params`.
  Nullary calls take `{}`. Grid defaults: 120 BPM, 4/4, sixteenth subdivision →
  one step = 125 000 µs, one bar = 16 steps.
- **Human action** column: the equivalent desktop edit/play interaction.

---

## Act 0 — Start & discover

| Beat | Agent | Human action | Expected |
|------|-------|--------------|----------|
| 0 | *(launch the app with `--control`; read the `hello` banner)* | open RockCraft | `hello` frame; the backend's one composer is empty (a new song) |
| 0a | `query {what:"Help"}` | `?` help overlay | catalog lists `attach_video`, `query_video`, `save_bundle`, `play_load`, `app_quit` |

## Act 1 — Add the movie as backing

| Beat | Agent | Human action | Expected |
|------|-------|--------------|----------|
| 1 | `run_host_command attach_video {path:"…/movie.mp4", offset_us:-100000}` | Edit screen → attach background video, nudge alignment | returns the `VideoRef` `{path, offset_us:-100000}` |
| — | `run_host_command query_video {}` | (the visible backdrop) | echoes the same reference |

## Act 2 — Author a short melody

| Beat | Agent | Human action | Expected |
|------|-------|--------------|----------|
| 2 | for each of (60,0) (62,4) (64,8) (65,12): `run_action set_cursor {pitch,step}` then `run_action add_note {}` | navigate + `a` four times | 4 notes on the timeline |
| — | `query {what:"State"}` | (the grid) | `state.notes.len() == 4` |
| 3 | `run_host_command query_dirty {}` | — | `true` (unsaved edits) |

## Act 3 — Save the song

| Beat | Agent | Human action | Expected |
|------|-------|--------------|----------|
| 4 | `run_host_command save_bundle {dest:{kind:"quick_save"}}` | Save | returns `{dir:"recordings/take-…"}`; writes `song.mid` + `meta.json` + a copy of the movie |
| 5 | `run_host_command query_dirty {}` | — | `false` (save cleared the flag); `meta.json` has a `video` block |

## Act 4 — Play it back

| Beat | Agent | Human action | Expected |
|------|-------|--------------|----------|
| 6 | `run_host_command play_load {dir:"recordings/take-…"}` | open the saved song in play mode | `PlayInfo` with `notes.len() == 4` and a non-null `video` (the movie rides into playback) |
| 7 | `run_host_command play_finish {}` | finish the take | a `PlaySummary` (the scoring loop ran) |

## Act 5 — Close the game

| Beat | Agent | Human action | Expected |
|------|-------|--------------|----------|
| 8 | `run_host_command app_quit {}` | close the window | the process exits; the socket closes (a reply or a clean close both count) |

---

## Keeping doc and driver in sync

The beats above are the section markers in
[`crates/control/examples/backing_movie_session.rs`](../crates/control/examples/backing_movie_session.rs).
When a beat changes, update both. The `attach_video` / `app_quit` family lives in
`crates/control/src/host.rs` (catalog + parity tests) and is dispatched by the
Tauri frontend in `tauri-app/src-tauri/src/control.rs`.

## See also

- [`AGENT-CONTROL.md`](AGENT-CONTROL.md) — the protocol, discovery, and the full
  host-command vocabulary
- [`DEMO-SCENARIO.md`](DEMO-SCENARIO.md) — the pure-composer action tour
