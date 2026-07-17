#!/usr/bin/env bash
# Runs the issue #12 token/latency benchmark and (optionally) checks the
# result against a committed baseline.
#
# Usage:
#   scripts/bench/run.sh [--repeats N] [--out PATH] [--check-against BASELINE]
#
# Examples:
#   scripts/bench/run.sh
#   scripts/bench/run.sh --repeats 50 --out /tmp/latest.json
#   scripts/bench/run.sh --check-against crates/codegen/simplicio-perf-bench/baselines/main.json
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$repo_root"

repeats=20
out_path="$(mktemp -t simplicio-bench-XXXXXX).json"
check_against=""

while [ $# -gt 0 ]; do
  case "$1" in
    --repeats)
      repeats="$2"; shift 2 ;;
    --out)
      out_path="$2"; shift 2 ;;
    --check-against)
      check_against="$2"; shift 2 ;;
    *)
      echo "unknown argument: $1" >&2
      exit 2 ;;
  esac
done

echo "Building benchmark binaries..." >&2
cargo build -q -p simplicio-perf-bench --bin simplicio-bench --bin simplicio-bench-check

echo "Running $repeats repeats per task..." >&2
cargo run -q -p simplicio-perf-bench --bin simplicio-bench -- --repeats "$repeats" --out "$out_path" >/dev/null

echo "Report written to: $out_path" >&2
cat "$out_path"

if [ -n "$check_against" ]; then
  echo "" >&2
  echo "Checking for regressions against $check_against ..." >&2
  cargo run -q -p simplicio-perf-bench --bin simplicio-bench-check -- \
    --baseline "$check_against" --current "$out_path"
fi
