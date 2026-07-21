#!/usr/bin/env python3
"""Enforce a deterministic line-coverage threshold from an LCOV report."""
from __future__ import annotations

import argparse
import sys
from pathlib import Path


def coverage_percent(report: Path) -> float:
    lines_found = 0
    lines_hit = 0
    for raw in report.read_text(encoding="utf-8").splitlines():
        if raw.startswith("LF:"):
            lines_found += int(raw[3:])
        elif raw.startswith("LH:"):
            lines_hit += int(raw[3:])
    if lines_found <= 0:
        raise ValueError("LCOV report contains no executable lines (LF=0)")
    return lines_hit * 100.0 / lines_found


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("report", type=Path)
    parser.add_argument("--threshold", type=float, required=True)
    args = parser.parse_args(argv)
    try:
        percent = coverage_percent(args.report)
    except (OSError, ValueError) as exc:
        print(f"coverage gate: invalid report: {exc}", file=sys.stderr)
        return 2
    print(f"coverage gate: {percent:.2f}% (required >= {args.threshold:.2f}%)")
    if percent < args.threshold:
        print("coverage gate: threshold not met", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
