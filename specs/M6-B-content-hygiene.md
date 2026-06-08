# M6-B — infra: repo content-hygiene (no media / charts in git)

> Milestone: M6 — Video Import · Issue: #114 · Suggested tier: sonnet
> Branch: `claude/m6-content-hygiene`

## Goal

Guarantee that **no source videos, audio files, or extracted `.mid` charts**
ever reach the public repo, while the *tool* itself is freely shared. Two
layers: gitignore rules + canonical gitignored output dirs, and a **CI guard
that fails the build** if disallowed content is ever committed.

## Context

- The repo is public; the import tool is fine to publish, the songs it produces
  are not (copyright). This must land **before** the extractor (M6-C) can
  produce anything, so there's never a window where output could be committed.
- Existing precedent: `/recordings/` is already gitignored; the curated
  `fixtures/` dir is the **allowlist** for intentionally-tracked test assets.
- The CI gate lives in `.github/workflows/ci.yml` (fmt · clippy · test). Add a
  guard step there without disturbing the existing job.
- Pairs with M6-A's `import_output_dir()` and M6-D's download cache.

## What to do

### 1. gitignore (`.gitignore`)

Add, with comments:
```
# Imported content — the TOOL is shared, the songs are NOT. Never commit these.
/import-out/           # extracted .mid chart bundles (M6-A output root)
/import-cache/         # downloaded / fetched source media
/scripts/local/        # private, machine-local fetch wrapper (yt-dlp etc.)
*.mp4
*.mkv
*.webm
*.mov
*.m4a
# (audio extensions *.mp3/*.wav/*.ogg/*.flac stay ignored too — backing-test.wav
#  and SoundFonts are already untracked; keep curated audio only under fixtures/)
```
Be careful not to ignore anything legitimately tracked; scope media globs and
prefer rooted dir ignores where possible.

### 2. Guard script — `scripts/check-no-media.sh`

- Lists **tracked** files (`git ls-files`) and **staged** files.
- Fails (non-zero, with a clear message naming the offending paths) if any match
  a disallowed pattern: the media extensions above, or a `*.mid`/`*.midi`
  **outside `fixtures/`**.
- The curated `fixtures/` tree is the only allowed home for tracked `.mid` /
  audio test assets.
- Usable both in CI and as a local pre-commit hook.

### 3. CI wiring (`.github/workflows/ci.yml`)

Add a step (e.g. before fmt) that runs `bash scripts/check-no-media.sh`. Keep
the existing fmt/clippy/test steps unchanged.

### 4. Optional local hook + docs

- `scripts/install-hooks.sh` that symlinks a pre-commit hook running the guard.
- `docs/IMPORT.md` (new) with a short **content policy**: the tool is shared;
  videos, audio, and extracted charts are personal and must never be committed;
  how the gitignore + CI guard enforce it; where downloading lives (the private
  hook, M6-D).

## Tests / verification

- Guard **fails** when a dummy `test.mp4` or an extracted `import-out/foo/song.mid`
  is staged; **passes** on the current clean tree; still **passes** with a
  `fixtures/midi/*.mid` present (allowlist works).
- CI run on the PR shows the new step green.
- `git check-ignore` confirms `import-out/`, `import-cache/`, `scripts/local/`,
  and a sample `*.mp4` are ignored.

## Scope boundaries (do NOT)

- Do not weaken or remove the existing fmt/clippy/test gate.
- Do not ignore or remove the curated `fixtures/` allowlist.
- No Rust code changes here; this is gitignore + scripts + CI + docs.

## Acceptance

- [ ] Guard script behaves as specified (fail/pass cases demonstrated in PR)
- [ ] CI guard step added and green; existing gate intact
- [ ] `docs/IMPORT.md` content policy written
- [ ] PR against `main` from `claude/m6-content-hygiene`, `Closes #114`
