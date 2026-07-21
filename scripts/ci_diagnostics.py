#!/usr/bin/env python3
"""Collect bounded, credential-safe diagnostics after a CI failure."""

from __future__ import annotations

import argparse
import json
import os
import platform
import re
import subprocess
import sys
from pathlib import Path
from typing import Any

SAFE_ENV = (
    "GITHUB_ACTIONS",
    "GITHUB_EVENT_NAME",
    "GITHUB_JOB",
    "GITHUB_RUN_ID",
    "GITHUB_SHA",
    "GITHUB_WORKFLOW",
)
SENSITIVE = re.compile(
    r"(?i)(token|secret|password|authorization|cookie|api[_-]?key)(\\s*[=:]\\s*)[^\\s,;]+"
)


def redact(value: str) -> str:
    return SENSITIVE.sub(r"\\1\\2[REDACTED]", value)[-4000:]


def run(command: list[str], root: Path) -> dict[str, Any]:
    try:
        result = subprocess.run(
            command,
            cwd=root,
            text=True,
            capture_output=True,
            timeout=30,
            check=False,
        )
    except (OSError, subprocess.TimeoutExpired) as error:
        return {"command": command, "status": "unavailable", "error": redact(str(error))}
    return {
        "command": command,
        "status": "passed" if result.returncode == 0 else "failed",
        "exit_code": result.returncode,
        "stdout": redact(result.stdout),
        "stderr": redact(result.stderr),
    }


def collect(root: Path) -> dict[str, Any]:
    tracked = [
        ".github/workflows/ci.yml",
        "Cargo.toml",
        "Cargo.lock",
        "rust-toolchain.toml",
    ]
    return {
        "schema": "simplicio.ci-diagnostics/v1",
        "platform": platform.platform(),
        "python": platform.python_version(),
        "github": {
            key: os.environ[key]
            for key in SAFE_ENV
            if os.environ.get(key)
        },
        "files": {
            name: {"exists": path.is_file(), "bytes": path.stat().st_size}
            for name in tracked
            if (path := root / name).exists()
        },
        "commands": [
            run(["git", "status", "--short", "--branch"], root),
            run(["git", "diff", "--check"], root),
            run([sys.executable, "-m", "py_compile", "scripts/ci_diagnostics.py"], root),
        ],
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = json.dumps(collect(args.root.resolve()), indent=2, sort_keys=True) + "\\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(report, encoding="utf-8")
    print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
