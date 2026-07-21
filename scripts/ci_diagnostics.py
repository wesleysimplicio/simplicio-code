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

MAX_OUTPUT_BYTES = 4000
SCHEMA = "simplicio.ci-diagnostics/v1"
SAFE_ENV = (
    "GITHUB_ACTIONS",
    "GITHUB_EVENT_NAME",
    "GITHUB_JOB",
    "GITHUB_RUN_ID",
    "GITHUB_SHA",
    "GITHUB_WORKFLOW",
)
SENSITIVE = re.compile(
    r"(?P<name>token|secret|password|authorization|cookie|api[_-]?key)"
    r"(?P<separator>\s*[=:]\s*)(?P<value>[^\s,;]+)",
    re.IGNORECASE,
)


def redact(value: str) -> str:
    redacted = SENSITIVE.sub(
        lambda match: (
            f"{match.group('name')}{match.group('separator')}[REDACTED]"
        ),
        value,
    )
    return redacted[-MAX_OUTPUT_BYTES:]


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
    report = {
        "schema": SCHEMA,
        "platform": platform.platform(),
        "python": platform.python_version(),
        "github": {
            key: redact(os.environ[key])
            for key in SAFE_ENV
            if os.environ.get(key)
        },
        "files": {
            name: (
                {"exists": True, "bytes": path.stat().st_size}
                if path.is_file()
                else {"exists": False}
            )
            for name in tracked
            if (path := root / name)
        },
        "commands": [
            run(["git", "status", "--short", "--branch"], root),
            run(["git", "diff", "--check"], root),
            run([sys.executable, "-m", "py_compile", "scripts/ci_diagnostics.py"], root),
        ],
    }
    validate_report(report)
    return report


def validate_report(report: dict[str, Any]) -> None:
    """Reject malformed or unredacted data before it becomes an artifact."""

    if report.get("schema") != SCHEMA:
        raise ValueError("unexpected diagnostics schema")
    if not isinstance(report.get("github"), dict):
        raise ValueError("github diagnostics must be an object")
    if not isinstance(report.get("files"), dict):
        raise ValueError("file diagnostics must be an object")
    commands = report.get("commands")
    if not isinstance(commands, list):
        raise ValueError("command diagnostics must be a list")
    for command in commands:
        if not isinstance(command, dict):
            raise ValueError("command diagnostic must be an object")
        for field in ("stdout", "stderr", "error"):
            value = command.get(field)
            if value is not None and len(value) > MAX_OUTPUT_BYTES:
                raise ValueError(f"{field} exceeds the diagnostics bound")

    serialized = json.dumps(report, sort_keys=True)
    for match in SENSITIVE.finditer(serialized):
        if match.group("value") != "[REDACTED]":
            raise ValueError("unredacted sensitive value in diagnostics")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    report = json.dumps(collect(args.root.resolve()), indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(report, encoding="utf-8")
    print(report, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
