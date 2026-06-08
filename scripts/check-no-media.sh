#!/usr/bin/env bash
# check-no-media.sh — CI guard and pre-commit hook
#
# Fails if any disallowed media/chart files are tracked or staged.
# Allowed exception: *.mid / *.midi / audio files inside fixtures/ (curated test assets).

set -euo pipefail

FAIL=0
OFFENDERS=()

MEDIA_EXTENSIONS='\.mp4$|\.mkv$|\.webm$|\.mov$|\.m4a$|\.mp3$|\.wav$|\.ogg$|\.flac$'
MIDI_EXTENSIONS='\.mid$|\.midi$'

# Collect tracked + staged files (deduplicated).
ALL_FILES=$(
  { git ls-files; git diff --cached --name-only --diff-filter=ACMRT; } \
    | sort -u
)

while IFS= read -r file; do
  [[ -z "$file" ]] && continue

  # Check media extensions (never allowed anywhere).
  if echo "$file" | grep -qiE "$MEDIA_EXTENSIONS"; then
    OFFENDERS+=("$file  [media file]")
    FAIL=1
    continue
  fi

  # Check .mid/.midi outside fixtures/.
  if echo "$file" | grep -qiE "$MIDI_EXTENSIONS"; then
    if ! echo "$file" | grep -q '^fixtures/'; then
      OFFENDERS+=("$file  [.mid outside fixtures/]")
      FAIL=1
    fi
  fi
done <<< "$ALL_FILES"

if [[ $FAIL -ne 0 ]]; then
  echo "ERROR: disallowed media/chart files detected in the repository:" >&2
  for f in "${OFFENDERS[@]}"; do
    echo "  $f" >&2
  done
  echo "" >&2
  echo "The TOOL is shared; the SONGS are not." >&2
  echo "  - Media files (video/audio) must never be committed." >&2
  echo "  - Extracted .mid charts belong in /import-out/ (gitignored), not in git." >&2
  echo "  - Curated test assets may live only under fixtures/." >&2
  echo "See docs/IMPORT.md for the full content policy." >&2
  exit 1
fi

echo "check-no-media: clean — no disallowed media or charts tracked."
