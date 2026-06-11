# Content Policy — Video Import

## The rule in plain language

**The tool is shared. The songs are not.**

RockCraft is open-source. Anyone can clone it, build it, and run it against
their own media. What they may not do — and what this repository must never
contain — is the actual video or audio files, or the `.mid` charts extracted
from them. Those are covered by copyright and must stay on your local machine.

## What is and isn't allowed in git

| Allowed | Not allowed |
|---------|-------------|
| Rust source, scripts, CI config, docs | Video files (`*.mp4`, `*.mkv`, `*.webm`, `*.mov`, `*.m4a`) |
| Curated test fixtures under `fixtures/` | Audio files (`*.mp3`, `*.wav`, `*.ogg`, `*.flac`) outside `fixtures/` |
| `fixtures/**/*.mid` (hand-crafted test MIDIs) | Extracted `.mid` charts anywhere outside `fixtures/` |
| Everything in `.gitignore` is already untracked | Anything in `/import-out/` or `/import-cache/` |

## How the guardrails work

### 1. `.gitignore`

The following paths and extensions are gitignored at the repo root so that
`git add .` will never accidentally stage them:

- `/import-out/` — where M6 writes extracted chart bundles
- `/import-cache/` — where M6's downloader caches source media
- `/scripts/local/` — your private fetch wrapper (e.g. a `yt-dlp` invocation)
- `*.mp4`, `*.mkv`, `*.webm`, `*.mov`, `*.m4a` — video containers
- Audio globs (`*.mp3`, `*.wav`, etc.) are **not** globally gitignored because
  `fixtures/` may contain small curated `.wav` / SoundFont test assets. The CI
  guard (see below) enforces the boundary instead.

### 2. CI guard (`scripts/check-no-media.sh`)

Every CI run executes this script before the Rust build. It scans all tracked
and staged files and **fails** if it finds:

- Any media extension (video or audio) anywhere in the tree.
- Any `.mid` / `.midi` file **outside** `fixtures/`.

A clean tree passes silently. The curated `fixtures/` directory is the only
allowed home for tracked `.mid` or audio test assets.

### 3. Local pre-commit hook (optional)

Run once after cloning to catch violations before they reach CI:

```sh
bash scripts/install-hooks.sh
```

This installs `.git/hooks/pre-commit` that runs the same guard on every
`git commit`.

## Backing audio (the source video's sound)

After the chart is extracted, the pipeline derives an audio track from the
source video so an imported song plays with its real recording behind it by
default. It shells out to **ffmpeg** (`ffmpeg -i <video> -vn backing.wav`),
writing `backing.wav` next to `song.mid` inside the bundle dir under
`import-out/` (gitignored, like the rest of the bundle), and records it in the
bundle's `meta.json` as `"backing"`. Play and Edit then pick it up automatically.

ffmpeg is an **optional** system dependency:

- If ffmpeg is installed (on `PATH`, or pointed at by `ROCKCRAFT_FFMPEG`), the
  import attaches the extracted audio as the backing track.
- If ffmpeg is absent — or the source has no audio stream — the import still
  succeeds; the bundle is simply MIDI-only (`"backing": null`).

The extracted `backing.wav` lives only under `import-out/` and is never tracked
(same boundary the CI guard enforces above).

## Where downloading lives

The actual media download step is intentionally **not** part of this repo.
It lives in `scripts/local/` on your machine (gitignored), where you can
place a private wrapper around `yt-dlp`, `ffmpeg`, or whatever tool you use
to fetch and prepare source files. M6-D documents the expected interface
between that wrapper and the import pipeline.

## Summary

- Clone, build, contribute to the tool freely.
- Keep your video, audio, and extracted charts on your own machine.
- `import-out/` and `import-cache/` are always gitignored.
- The CI guard is the last line of defense — it will reject any PR that
  accidentally contains disallowed content.
