# M18-D — TUI scoring

> Milestone: M18 · Issue: #299 · Suggested tier: opus
> Branch: `claude/tui-scoring`
> Depends on: M18-C (#298, practice hand) — score only the practised hand. If M18-C
> hasn't landed, score every note and leave a `practice` hook.

## Goal

Score the TUI player's playing like the desktop app: a live score and combo
while playing, a summary at the end of the song, and a score for each *your
turn* pass of the practice loop.

## Context

- Pure scoring lives in `core`: `score(expected, played, ScoreConfig) ->
  ScoreReport` (`crates/core/src/scoring.rs`; default windows 50 ms perfect,
  150 ms good) and `Summary::from_report` (`crates/core/src/stats.rs`).
- Desktop reference, to mirror closely: `tauri-app/src-tauri/src/play.rs` —
  `ingest` (collects strikes), `score_due` (judges a note once its good-window
  has closed), `recompute_live`, `award` (perfect 100 + 2·combo, good 50 +
  combo, a miss resets the combo), `report` / `finish` / `PlaySummary`, and in
  a loop `RunningLoop.played` / `scored`, `finish_pass`, `PassSummary`,
  `last_pass`. `tick_play` shows why live strikes are **re-stamped with the
  play-clock time** before scoring (device time keeps running while the clock
  is frozen or slowed).
- TUI: `crates/tui/src/play.rs`. Live keys reach it through `ingest` (the shell
  sounds them) or `track_held` (the MIDI thread already sounded them) — both
  must collect strikes. At the song end the shell currently returns to the menu
  with "song finished" (`app.rs`).
- Host commands `play_status` and `play_finish` exist; the TUI answers
  `Unsupported`.

## Behaviour

- **Strikes:** every note-on from the player is recorded at the play clock's
  current time (not the device timestamp). Note-offs don't score.
- **Live judging:** a note is judged once the clock passes `start + good_us`;
  the live score and combo update then, exactly as the desktop's `score_due` /
  `recompute_live` (so the live numbers equal the final report).
- **What is scored:** outside a loop, every note of the practised hand (all
  notes with both hands). After a seek (`←` / `→`), notes after the target are
  un-judged and strikes after it dropped, as the desktop's `seek_to` does.
- **Practice loop:** only *your turn* scores (strikes in its count-in count, for
  an early first note), only notes starting in the loop, and each pass scores
  on its own. At the pass end the pass is summarised into `last_pass`
  (`pass, hits, misses, accuracy`), and the live score/combo reset for the next
  pass. The loop badge shows the last pass: `Loop 5–8 · DEMO · pass 3 · last 92%`.
- **Status line:** `score 12 340  combo 17` next to the clock.
- **End of song:** instead of dropping to the menu, show a summary screen:
  hits / misses / extras, perfect / early / late, accuracy %, best combo, score.
  `r` plays again, `Tab` / `Esc` goes to the menu.
- **Host commands:** `play_status` returns the live take (clock, paused, wait
  mode and what it awaits, practice hand if M18-C landed, rate if M18-B landed,
  score, combo, hits, misses, bar, practice loop) — the same field names as the
  desktop's `PlayStatusView` where they exist. `play_finish` ends the take and
  returns the summary (the desktop's `PlaySummary` fields). Both fail cleanly
  off the play screen.

## Tests

(Headless, driving `advance` and `ingest` / `track_held` with explicit times.)
- A perfect take: all hits, accuracy 100%, score = the desktop's formula for the
  same chart; early/late strikes inside the good window count as hits, not
  perfect.
- A missed note is judged a miss only once its window has closed, and resets
  the combo; an extra strike is an extra.
- Strikes are stamped with the play clock: a strike during a wait-mode freeze
  lands on the awaited note.
- `track_held` (MIDI-thread echo) scores the same as `ingest`.
- Live score/combo after the last note equal the final summary.
- Seeking back un-judges the notes after the target.
- Loop: demo strikes don't score; a perfect pass and a pass with one miss give
  the right `last_pass`; each pass starts from zero.
- Summary screen: shown at the song end; `r` replays; `Tab` returns to the menu.
- `play_status` / `play_finish` on and off the play screen.

## Scope boundaries (do NOT)

- No per-note hit effects on the highway (a later task).
- No saving scores or high scores to disk.
- No changes to `core` scoring rules or the desktop app.
- No new dependencies.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #299`
