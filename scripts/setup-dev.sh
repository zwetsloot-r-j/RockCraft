#!/usr/bin/env bash
# Install the system libraries RockCraft needs to *build* (not just to run).
#
# The `audio` and `tui` crates link ALSA via `alsa-sys` (pulled in by rodio and
# midir). Without the ALSA development headers, `cargo build`/`cargo test`
# aborts inside `alsa-sys`'s build script with a cryptic:
#
#     pkg-config exited with status code 1
#     Package alsa was not found in the pkg-config search path.
#
# This is the same step CI runs (.github/workflows/ci.yml). Safe to re-run.
#
# Pure `core` work needs none of this — scope to `cargo test -p rockcraft-core`.
set -euo pipefail

if command -v apt-get >/dev/null 2>&1; then
    sudo apt-get update
    sudo apt-get install -y libasound2-dev pkg-config
else
    echo "Non-apt system detected. Install the ALSA development headers and" >&2
    echo "pkg-config with your package manager, e.g.:" >&2
    echo "  Fedora/RHEL:  sudo dnf install alsa-lib-devel pkgconf-pkg-config" >&2
    echo "  Arch:         sudo pacman -S alsa-lib pkgconf" >&2
    exit 1
fi

echo "RockCraft build deps installed: libasound2-dev, pkg-config."
