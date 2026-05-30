# <id> — <Title>

> Milestone: <M0 / M1 / …> · Issue: #<n> · Suggested tier: <opus/sonnet/cheap>
> Branch: `<claude|vibe|feat>/<slug>`

## Goal

One or two sentences: what this task delivers and why.

## Context

Pointers the agent needs: which crate, which existing types/functions it builds
on, links to related specs (`specs/...`). Remember the agent reads `CLAUDE.md` /
`AGENTS.md` for architecture invariants — don't repeat them, just reference.

## What to do

Precise, testable steps. Prefer exact signatures over prose, e.g.:

```rust
// in crates/<crate>/src/<file>.rs
pub fn thing(...) -> ...   // behaviour, edge cases, examples
```

Spell out edge cases with concrete expected values.

## Tests

The specific cases the unit tests must cover (concrete inputs → outputs).

## Scope boundaries (do NOT)

- Do not change any other file / any existing public signature unless listed.
- Do not add third-party dependencies unless this spec explicitly says to.
- <task-specific don'ts>

## Acceptance

- [ ] `cargo fmt --all --check` clean
- [ ] `cargo clippy --workspace --all-targets` clean (warnings are errors)
- [ ] `cargo test --workspace` green
- [ ] PR opened against `main` from the branch above, `Closes #<n>`
