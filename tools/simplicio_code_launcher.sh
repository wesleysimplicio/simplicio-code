#!/usr/bin/env bash
# Local underscore-compatible launcher for the Simplicio Code TUI.
#
# `simplicio_code` is intentionally distinct from the Runtime command
# `simplicio` and from the Agent command `simplicio_agent`.
set -euo pipefail

SOURCE="${BASH_SOURCE[0]}"
while [[ -L "$SOURCE" ]]; do
  SOURCE_DIR="$(cd -P "$(dirname "$SOURCE")" && pwd)"
  SOURCE="$(readlink "$SOURCE")"
  [[ "$SOURCE" = /* ]] || SOURCE="$SOURCE_DIR/$SOURCE"
done
SCRIPT_DIR="$(cd -P "$(dirname "$SOURCE")" && pwd)"
REPO_ROOT="${SIMPLICIO_CODE_REPO:-$(cd "$SCRIPT_DIR/.." && pwd)}"

if [[ -n "${SIMPLICIO_CODE_BIN:-}" ]]; then
  BIN="$SIMPLICIO_CODE_BIN"
elif [[ -x "$REPO_ROOT/target/release/simplicio-code" ]]; then
  BIN="$REPO_ROOT/target/release/simplicio-code"
elif [[ -x "$REPO_ROOT/target/debug/simplicio-code" ]]; then
  BIN="$REPO_ROOT/target/debug/simplicio-code"
else
  echo "simplicio_code: binary not found; build with:" >&2
  echo "  cargo build -p xai-grok-pager-bin --bin simplicio-code" >&2
  exit 1
fi

exec "$BIN" "$@"
