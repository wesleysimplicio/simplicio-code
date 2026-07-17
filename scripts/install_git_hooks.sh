#!/usr/bin/env bash
# simplicio-code (issue #9): install the tracked git hooks into .git/hooks/.
# Idempotent -- safe to re-run after every clone/checkout/pull.
set -euo pipefail
DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
if [ ! -d "$DIR/.git" ]; then
  echo "install_git_hooks: no .git directory found at $DIR -- not a git checkout, nothing to do."
  exit 0
fi
for hook in pre-commit pre-push; do
  cp "$DIR/scripts/git-hooks/$hook" "$DIR/.git/hooks/$hook"
  chmod +x "$DIR/.git/hooks/$hook"
  echo "installed $hook hook -> $DIR/.git/hooks/$hook"
done
