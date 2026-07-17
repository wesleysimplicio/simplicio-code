#!/usr/bin/env bash
# simplicio-code (issue #9): install the tracked git hooks into .git/hooks/.
# Idempotent -- safe to re-run after every clone/checkout/pull.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SRC="$DIR/scripts/git-hooks/pre-commit"
DEST="$DIR/.git/hooks/pre-commit"
if [ ! -d "$DIR/.git" ]; then
  echo "install_git_hooks: no .git directory found at $DIR -- not a git checkout, nothing to do."
  exit 0
fi
cp "$SRC" "$DEST"
chmod +x "$DEST"
echo "installed pre-commit hook (secret-scan + plugin sync) -> $DEST"
