#!/usr/bin/env bash
# install-hooks.sh — wire check-no-media.sh as a local git pre-commit hook.
#
# Run once after cloning:  bash scripts/install-hooks.sh

set -euo pipefail

REPO_ROOT="$(git rev-parse --show-toplevel)"
HOOK="$REPO_ROOT/.git/hooks/pre-commit"

cat > "$HOOK" << 'EOF'
#!/usr/bin/env bash
bash "$(git rev-parse --show-toplevel)/scripts/check-no-media.sh"
EOF

chmod +x "$HOOK"
echo "Pre-commit hook installed at $HOOK"
