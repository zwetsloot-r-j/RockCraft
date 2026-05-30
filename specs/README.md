# specs/

Detailed task specifications, one Markdown file per task. These are the
**payload** an agent builds against; the matching **GitHub Issue** is the
pointer + state (labels, assignee, status, linked PR).

## Why specs live here (not only in the issue)

Cloud sandboxes (Claude Code web, Vibe Code Web) clone the repo, so a spec
committed here is automatically in the agent's context — no connector fetch
needed — and it's version-controlled alongside the code it describes.

## Flow

1. Write the spec: `specs/<id>-<slug>.md` (e.g. `specs/M0-echo.md`).
2. Open a GitHub Issue that links to it; apply routing labels
   (`agent:*`, `area:*`, `tier:*`, and `loc:local` if it needs the piano)
   and a milestone.
3. Delegate: paste the issue/spec link (or the spec text) into the chosen
   vendor's sandbox, on the right branch prefix (`claude/*`, `vibe/*`,
   `feat/*` for human).
4. The PR closes the issue (`Closes #N`) once the gate is green.

## What a good spec contains

See [`_TEMPLATE.md`](_TEMPLATE.md). The non-negotiables that kept our canary
PRs clean: an explicit **scope boundary** ("do not change X / do not add
deps") and **acceptance = the CI gate passes** (`fmt · clippy · test`).
