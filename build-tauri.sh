#!/usr/bin/env bash
# Build the Tauri desktop app as a **Windows** binary, from a WSL checkout.
#
#   ./build-tauri.sh            # build
#   ./build-tauri.sh --release  # optimised build
#
# Output: target-win/<profile>/rockcraft-tauri.exe  (run it with ./run-tauri.sh)
#
# Two things this exists to get right:
#
#  1. It is a TWO-step build. `custom-protocol` embeds the frontend into the exe
#     at *compile* time via `generate_context!()`, so a frontend-only change
#     needs `vite build` AND a Rust recompile. Cargo will not re-expand the macro
#     unless a `.rs` file changed, so we touch `lib.rs` — without that the exe
#     silently keeps the OLD frontend, which looks exactly like your change not
#     working.
#  2. Without `--features tauri/custom-protocol` the webview loads from a dev
#     server that isn't running, and the window shows "localhost refused to
#     connect".
#
# The final step verifies the embedded bundle hash matches the one vite just
# wrote, so a stale embed is caught here rather than after ten minutes of
# wondering why the fix didn't take.
set -euo pipefail

cd "$(dirname "$0")"

PROFILE="debug"
CARGO_ARGS=()
for arg in "$@"; do
  case "$arg" in
    --release) PROFILE="release"; CARGO_ARGS+=("--release") ;;
    *) CARGO_ARGS+=("$arg") ;;
  esac
done

# Windows cargo — the Linux one would produce an ELF binary that WSLg runs
# without a native webview. $CARGO_EXE overrides if yours lives elsewhere.
find_cargo() {
  if [[ -n "${CARGO_EXE:-}" ]]; then echo "$CARGO_EXE"; return; fi
  if command -v cargo.exe >/dev/null 2>&1; then command -v cargo.exe; return; fi
  for c in /mnt/c/Users/*/.cargo/bin/cargo.exe; do
    [[ -x "$c" ]] && { echo "$c"; return; }
  done
  echo ""
}
CARGO="$(find_cargo)"
if [[ -z "$CARGO" ]]; then
  echo "error: no Windows cargo.exe found." >&2
  echo "       Install Rust on Windows, or set CARGO_EXE=/mnt/c/.../cargo.exe" >&2
  exit 1
fi

echo "▸ 1/3  frontend (vite build)"
(cd tauri-app && npx vite build)

# See the header: cargo will not re-embed a changed frontend on its own.
echo "▸ 2/3  touch lib.rs so the frontend is re-embedded"
touch tauri-app/src-tauri/src/lib.rs

echo "▸ 3/3  Windows binary ($PROFILE) via $CARGO"
"$CARGO" build -p rockcraft-tauri \
  --features tauri/custom-protocol \
  --target-dir target-win \
  "${CARGO_ARGS[@]}"

EXE="target-win/$PROFILE/rockcraft-tauri.exe"
BUNDLE="$(ls -t tauri-app/dist/assets/index-*.js 2>/dev/null | head -1 || true)"
if [[ -n "$BUNDLE" ]] && ! grep -aq "$(basename "$BUNDLE" .js)" "$EXE"; then
  echo "warning: '$(basename "$BUNDLE")' is not embedded in the exe — stale frontend." >&2
  echo "         Re-run this script; if it persists, the exe may have been locked" >&2
  echo "         by a running app (close it first)." >&2
  exit 1
fi

echo
echo "✓ built $EXE  (frontend $(basename "${BUNDLE:-?}") embedded)"
echo "  run it with:  ./run-tauri.sh"
