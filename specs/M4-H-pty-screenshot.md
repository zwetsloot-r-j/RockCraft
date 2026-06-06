# M4-H — infra: real pixel screenshot via headless PTY (stretch)

> Milestone: M4 — Agent Interface · Issue: #93 · Suggested tier: sonnet
> Branch: `claude/m4-pty-screenshot`

## Goal

Optional visual-verification tier: run the real TUI binary inside a headless
pseudo-terminal, capture the rendered frame, and produce a PNG an agent (or a
human reviewing a PR) can look at. Complements M4-G's text dump for cases where
true visuals matter — and it's the path that carries over when Tauri/Godot
frontends arrive.

## Context

- Mostly `crates/control` (a small capture utility / example) + `area:infra`.
  Builds on M4-F/M4-G (#91/#92): the agent drives edits over the socket, then
  asks for a screenshot.
- This is a **stretch** task — land M4-A…G first. It is *not* `loc:local`: a PTY
  runs fine in CI; no piano needed.
- New deps (this spec authorises, confined to a non-default feature or a separate
  example so the core build stays lean): a PTY crate (e.g. `portable-pty`) and a
  terminal-grid-to-image renderer, or shell out to an external `*-to-png` tool.

## What to do

- Provide a utility (binary or `cargo run --example screenshot`) that:
  1. launches the TUI with `--control` on a known port in a PTY of fixed size;
  2. optionally replays a small action script over the socket;
  3. captures the terminal grid and writes `screenshot.png` (+ the M4-G text dump
     alongside, for diffing).
- Document determinism caveats: fix terminal size, disable blinking/animation,
  prefer the text dump for assertions and the PNG for human/agent eyeballing.
- Gate any heavy deps behind a feature (e.g. `screenshot`) so the default
  workspace build/test is unaffected.

## Tests

- A smoke test (feature-gated) that the utility launches the app, captures a
  non-empty image of the requested dimensions, and exits cleanly. Keep it out of
  the default `cargo test --workspace` path if the deps are heavy; document how to
  run it.

## Scope boundaries (do NOT)

- Do not make screenshotting a dependency of the normal build/test gate.
- Do not change `core`, the protocol, or the app beyond what M4-F/G expose.
- Do not block on this for the rest of M4 — it is the last, optional piece.

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings = errors)
- [ ] `cargo test --workspace` green (screenshot smoke test feature-gated)
- [ ] PR against `main` from `claude/m4-pty-screenshot`, `Closes #93`
