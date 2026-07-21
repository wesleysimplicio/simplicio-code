#!/usr/bin/env python3
"""Emit safe, reproducible diagnostics when CI setup fails.

The report intentionally contains versions, repository state and command
results only. Environment values are never dumped because CI setup failures
must remain useful without risking credentials in uploaded artifacts.
"""

from __future__ import annotations

import argparse
import json
import os
import platform
import subprocess
import sys
from pathlib import Path
from typing import Any


def _run(command: list[str], cwd: Path) -> dict[str, Any]:
    try:
        completed = subprocess.run(
            command,
            cwd=cwd,
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return {"command": command, "status": "unavailable", "error": str(exc)}
    return {
        "command": command,
        "status": "passed" if completed.returncode == 0 else "failed",
        "exit_code": completed.returncode,
        "stdout": completed.stdout[-4000:],
        "stderr": completed.stderr[-4000:],
    }


def collect(root: Path) -> dict[str, Any]:
    files = [
        ".github/workflows/ci.yml",
        ".github/workflows/json-boundaries.yml",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
    ]
    return {
        "schema": "simplicio.ci-diagnostics/v1",
        "platform": platform.platform(),
        "python": sys.version,
        "github": {
            key: os.environ.get(key)
            for key in ("GITHUB_ACTIONS", "GITHUB_EVENT_NAME", "GITHUB_RUN_ID", "GITHUB_SHA")
            if os.environ.get(key)
        },
        "files": {
            name: {"exists": (root / name).is_file(), "bytes": (root / name).stat().st_size}
            for name in files
            if (root / name).exists()
        },
        "commands": [
            _run(["git", "status", "--short", "--branch"], root),
            _run(["git", "diff", "--check"], root),
            _run([sys.executable, "-m", "py_compile", "scripts/ci_diagnostics.py"], root),
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = collect(args.root.resolve())
    encoded = json.dumps(report, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
