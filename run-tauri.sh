#!/usr/bin/env bash
# Run the Tauri desktop app on the Windows host, from a WSL checkout.
#
#   ./run-tauri.sh              # run (window opens on Windows)
#   ./run-tauri.sh --release    # run the optimised build
#   ./run-tauri.sh --fg         # stay in the foreground (Ctrl-C to quit)
#
# Anything after those flags is passed straight to the app.
#
# The WSLENV line is the thing worth knowing: a plain `FOO=bar ./app.exe` does
# NOT reach a Windows process launched from WSL, so the control socket would
# silently never start. WSLENV is what forwards the variables across, and the
# `/p` suffix translates a path from WSL form to Windows form.
#
# The control socket defaults on at 127.0.0.1:9001 so an agent (or
# scripts/local/rc.mjs) can drive the app. Set ROCKCRAFT_CONTROL_ADDR yourself to
# move it, or NO_CONTROL=1 to start without it.
set -euo pipefail

cd "$(dirname "$0")"

PROFILE="debug"
FOREGROUND=0
ARGS=()
for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release" ;;
    --fg|--foreground) FOREGROUND=1 ;;
    *) ARGS+=("$arg") ;;
  esac
done

EXE="target-win/$PROFILE/rockcraft-tauri.exe"
if [[ ! -x "$EXE" ]]; then
  echo "error: $EXE not found — build it first:" >&2
  echo "         ./build-tauri.sh$([[ $PROFILE == release ]] && echo ' --release')" >&2
  exit 1
fi

# The app locks its own exe; a second copy also exits immediately (single
# instance). Say so plainly rather than letting it look like a silent failure.
# Two traps here, both of which made this check silently never match:
#   - `tasklist.exe 2>/dev/null` yields no output at all under WSL (redirecting
#     its stderr loses stdout with it), so it must be 2>&1.
#   - piping into `grep -q` under `set -o pipefail` *inverts* the result: grep
#     exits at the first match, SIGPIPEs tasklist, and the failed writer makes
#     the pipeline non-zero precisely when it matched. So capture, then grep.
RUNNING=""
if command -v tasklist.exe >/dev/null 2>&1; then
  RUNNING="$(tasklist.exe 2>&1 || true)"
fi
if [[ "$RUNNING" == *rockcraft-tauri.exe* ]]; then
  echo "note: RockCraft is already running — bringing that window forward, not starting a second."
  exit 0
fi

FORWARD=()
if [[ "${NO_CONTROL:-}" != "1" ]]; then
  export ROCKCRAFT_CONTROL_ADDR="${ROCKCRAFT_CONTROL_ADDR:-127.0.0.1:9001}"
  FORWARD+=("ROCKCRAFT_CONTROL_ADDR")
fi

# Audio needs a SoundFont: without one the whole audio thread dies, taking the
# backing track with it, not just the synth. Only forwarded if present.
SF2_DEFAULT="$PWD/crates/audio/assets/piano.sf2"
if [[ -z "${ROCKCRAFT_SF2:-}" && -f "$SF2_DEFAULT" ]]; then
  export ROCKCRAFT_SF2="$SF2_DEFAULT"
fi
if [[ -n "${ROCKCRAFT_SF2:-}" ]]; then
  FORWARD+=("ROCKCRAFT_SF2/p")   # /p = translate the path to Windows form
else
  echo "note: no SoundFont at crates/audio/assets/piano.sf2 — audio will be silent."
fi

# Join with ':' and merge with any WSLENV the caller already set.
JOINED="$(IFS=:; echo "${FORWARD[*]:-}")"
if [[ -n "$JOINED" ]]; then
  export WSLENV="${WSLENV:+$WSLENV:}$JOINED"
fi

echo "▸ starting $EXE"
[[ -n "${ROCKCRAFT_CONTROL_ADDR:-}" ]] && echo "  control socket: ws://$ROCKCRAFT_CONTROL_ADDR"

if [[ $FOREGROUND -eq 1 ]]; then
  exec "./$EXE" "${ARGS[@]}"
fi

LOG="${TMPDIR:-/tmp}/rockcraft-tauri.log"
"./$EXE" "${ARGS[@]}" >"$LOG" 2>&1 &
PID=$!
sleep 2
if ! kill -0 "$PID" 2>/dev/null; then
  echo "error: the app exited immediately. Output:" >&2
  cat "$LOG" >&2
  exit 1
fi
echo "  running in the background (log: $LOG)"
