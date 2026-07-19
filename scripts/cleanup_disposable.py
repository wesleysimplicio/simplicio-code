#!/usr/bin/env python3
"""Explicit cleanup of the disposable Cargo target directory (#52).

This helper is deliberately separate from the read-only disk preflight. It
only considers the exact ``<workspace>/target`` child, refuses symlinks, and
requires both ``--delete`` and ``SIMPLICIO_ALLOW_DISPOSABLE_CLEANUP=1``.
No git branch, worktree, source file, Cargo registry, or ``.simplicio`` state
is touched.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import sys
from pathlib import Path


def size_bytes(path: Path) -> int:
    total = 0
    if not path.is_dir() or path.is_symlink():
        return 0
    for child in path.rglob("*"):
        try:
            if child.is_file() and not child.is_symlink():
                total += child.stat().st_size
        except OSError:
            continue
    return total


def cleanup(workspace: Path, delete: bool, environ: dict[str, str] | None = None) -> dict[str, object]:
    environ = os.environ if environ is None else environ
    target = (workspace / "target").resolve(strict=False)
    expected_parent = workspace.resolve(strict=True)
    report: dict[str, object] = {
        "schema": "simplicio.disposable-cleanup/v1",
        "workspace": str(expected_parent),
        "path": str(target),
        "status": "planned",
        "bytes_before": 0,
        "bytes_reclaimed": 0,
    }
    raw_target = workspace / "target"
    if raw_target.is_symlink():
        report.update(status="refused_symlink", reason="target must not be a symlink")
        return report
    if target.parent != expected_parent:
        report.update(status="refused_path", reason="resolved target escaped workspace")
        return report
    if not raw_target.exists():
        report.update(status="nothing_to_clean")
        return report
    report["bytes_before"] = size_bytes(raw_target)
    if not delete:
        report["reason"] = "dry run; pass --delete to request removal"
        return report
    if environ.get("SIMPLICIO_ALLOW_DISPOSABLE_CLEANUP") != "1":
        report.update(
            status="blocked_confirmation",
            reason="set SIMPLICIO_ALLOW_DISPOSABLE_CLEANUP=1 together with --delete",
        )
        return report
    shutil.rmtree(raw_target)
    report.update(status="deleted", bytes_reclaimed=report["bytes_before"])
    return report


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Plan or explicitly remove workspace/target")
    parser.add_argument("--workspace", type=Path, default=Path.cwd())
    parser.add_argument("--delete", action="store_true")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    report = cleanup(args.workspace, args.delete)
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else report["status"])
    return 0 if report["status"] in {"planned", "nothing_to_clean", "deleted"} else 2


if __name__ == "__main__":
    raise SystemExit(main())
