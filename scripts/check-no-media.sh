#!/usr/bin/env bash
# check-no-media.sh — CI guard and pre-commit hook
#
# Fails if any disallowed media/chart/score files are tracked or staged.
# Allowed exception: *.mid / *.midi / audio / score-source files inside fixtures/
# (curated test assets).

set -euo pipefail

FAIL=0
OFFENDERS=()

MEDIA_EXTENSIONS='\.mp4$|\.mkv$|\.webm$|\.mov$|\.m4a$|\.mp3$|\.wav$|\.ogg$|\.flac$'
MIDI_EXTENSIONS='\.mid$|\.midi$'

# Sheet music is copyrighted exactly like a recording (M13). Two tiers:
#
# Never allowed anywhere — either a finished publication or an opaque container
# no reviewer can read in a diff (.mxl is a zip; .mscz/.sib/.gp* are binary):
SCORE_BINARY_EXTENSIONS='\.pdf$|\.mxl$|\.mscz$|\.sib$|\.gp3$|\.gp4$|\.gp5$|\.gpx$'
# Allowed only under fixtures/ — plain-text score sources, reviewable in a diff:
SCORE_TEXT_EXTENSIONS='\.musicxml$|\.abc$|\.krn$'
# `.xml` is the generic XML extension — the repo tracks Android launcher icons
# and other unrelated XML — so it is judged by content, not by name: a file whose
# root element is MusicXML's is a score and gets the same fixtures/-only rule.
SCORE_XML_ROOT='<score-partwise|<score-timewise'

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

  # Check binary/published score formats (never allowed anywhere).
  if echo "$file" | grep -qiE "$SCORE_BINARY_EXTENSIONS"; then
    OFFENDERS+=("$file  [published/opaque score file]")
    FAIL=1
    continue
  fi

  # Check .mid/.midi outside fixtures/.
  if echo "$file" | grep -qiE "$MIDI_EXTENSIONS"; then
    if ! echo "$file" | grep -q '^fixtures/'; then
      OFFENDERS+=("$file  [.mid outside fixtures/]")
      FAIL=1
    fi
    continue
  fi

  # Check plain-text score sources outside fixtures/.
  if echo "$file" | grep -qiE "$SCORE_TEXT_EXTENSIONS"; then
    if ! echo "$file" | grep -q '^fixtures/'; then
      OFFENDERS+=("$file  [score source outside fixtures/]")
      FAIL=1
    fi
    continue
  fi

  # A generic .xml is only a score if it says so. Peek at the head of the file
  # (a MusicXML root element appears within the first few lines).
  if echo "$file" | grep -qiE '\.xml$' && ! echo "$file" | grep -q '^fixtures/'; then
    if [[ -f "$file" ]] && head -c 4096 "$file" | grep -qiE "$SCORE_XML_ROOT"; then
      OFFENDERS+=("$file  [MusicXML score outside fixtures/]")
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
  echo "  - Published or opaque scores (.pdf/.mxl/.mscz/.sib/.gp*) must never be" >&2
  echo "    committed; .mxl is a zip container, so it is unreviewable in a diff." >&2
  echo "  - Extracted .mid charts belong in /import-out/ (gitignored), not in git." >&2
  echo "  - Curated test assets — .mid, audio, and plain-text scores" >&2
  echo "    (.musicxml/.xml/.abc/.krn) — may live only under fixtures/." >&2
  echo "See docs/IMPORT.md for the full content policy." >&2
  exit 1
fi

echo "check-no-media: clean — no disallowed media or charts tracked."
