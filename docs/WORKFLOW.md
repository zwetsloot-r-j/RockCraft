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
- `status:in-progress` — claimed by an agent/human; others skip it. Set by the
  worker when it starts, so a label-based queue doesn't get double-worked.

## Branch prefixes

`claude/*`, `vibe/*`, `feat/*` (human). One PR per branch, scoped to one task.

## Delegating: two ways

- **Specific issue:** "Work on issue #N." The agent reads the issue → its linked
  spec → does it.
- **Label queue:** "Work on issues labeled `tier:cheap`" (or `agent:vibe`, etc.).
  The agent lists open issues with that label, skips `loc:local` and
  `status:in-progress`, takes the lowest-numbered remaining one, and does it.

The exact steps an agent follows are in `CLAUDE.md` / `AGENTS.md` ("Picking up
work"). To make a queue safe, ensure queue issues are well-specced and not
`loc:local`.

## Lifecycle

1. Write `specs/<id>-<slug>.md` (copy `specs/_TEMPLATE.md`).
2. Open an Issue (Task template) linking the spec; apply labels + milestone.
3. Delegate by issue number or by label queue (see above).
4. The agent claims the issue (`status:in-progress`), works on its branch
   prefix, opens a PR with `Closes #N`.
5. Merges only when the gate (`fmt · clippy · test`) is green and `main`'s
   protection is satisfied. A human reviews; agents do not self-merge.

## What can be delegated to a sandbox

Anything verifiable by `cargo test` without hardware: `core`, file parsing,
scoring (against committed `fixtures/midi/`), refactors. Anything `loc:local`
stays with a human at the piano. See `CLAUDE.md` for architecture invariants.

For agent-driven editing via the WebSocket control interface, see
[`AGENT-CONTROL.md`](AGENT-CONTROL.md).
