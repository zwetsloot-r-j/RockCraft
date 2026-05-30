# Development workflow

How work is tracked and delegated across humans and AI agents (Claude, Vibe).

## The two layers

| Layer | Lives in | Holds |
|---|---|---|
| **Control plane** | GitHub Issues | what to do, owner, status, labels, linked PR |
| **Payload** | `specs/*.md` in the repo | the detailed, versioned spec the agent builds against |

A sandbox clones the repo, so the spec travels with it; the issue is how you
triage and route (including from mobile, away from the piano).

## Labels (routing)

- `agent:claude` / `agent:vibe` / `agent:human` — who owns it.
- `area:core` / `area:midi` / `area:audio` / `area:tui` / `area:infra` — which
  crate / surface.
- `loc:local` — needs the physical piano; **cannot** go to a cloud sandbox.
- `tier:opus` / `tier:sonnet` / `tier:cheap` — suggested model tier
  (Opus: design/concurrency; Sonnet: specced features; cheap: mechanical).

## Branch prefixes

`claude/*`, `vibe/*`, `feat/*` (human). One PR per branch, scoped to one task.

## Lifecycle

1. Write `specs/<id>-<slug>.md` (copy `specs/_TEMPLATE.md`).
2. Open an Issue (Task template) linking the spec; apply labels + milestone.
3. Delegate to the right vendor sandbox on the right branch prefix.
4. PR with `Closes #N`; merges only when the gate (`fmt · clippy · test`) is
   green and `main`'s protection is satisfied.

## What can be delegated to a sandbox

Anything verifiable by `cargo test` without hardware: `core`, file parsing,
scoring (against committed `fixtures/midi/`), refactors. Anything `loc:local`
stays with a human at the piano. See `CLAUDE.md` for architecture invariants.
