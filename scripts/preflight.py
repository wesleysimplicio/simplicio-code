#!/usr/bin/env python3
"""Fail-closed toolchain preflight for Simplicio Code (#58).

The preflight inventories every ``simplicio-dev-cli`` candidate visible in
PATH (or one explicit candidate), records the exact path and command used, and
never treats a missing/invalid version as ``0.0.0``.  It also verifies the
repository artifacts required by the Runtime contract and, when available,
runs the Runtime contract smoke command.
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from typing import Callable, Sequence

REQUIRED_ARTIFACTS = (
    "docs/SIMPLICIO_OPERATIONAL_MANUAL.md",
    "examples/EXAMPLES.md",
)


@dataclass(frozen=True)
class CommandResult:
    returncode: int
    stdout: str = ""
    stderr: str = ""


@dataclass(frozen=True)
class DevCliCandidate:
    path: str
    status: str
    version: str | None
    version_command: list[str]
    task_surface: bool
    detail: str

    def to_dict(self) -> dict[str, object]:
        return {
            "path": self.path,
            "status": self.status,
            "version": self.version,
            "version_command": self.version_command,
            "task_surface": self.task_surface,
            "detail": self.detail,
        }


def run_command(path: str, args: Sequence[str]) -> CommandResult:
    try:
        completed = subprocess.run(
            [path, *args], capture_output=True, text=True, timeout=15, check=False
        )
    except (OSError, subprocess.TimeoutExpired) as exc:
        return CommandResult(127, "", str(exc))
    return CommandResult(completed.returncode, completed.stdout, completed.stderr)


def _extract_version(payload: object) -> str | None:
    if isinstance(payload, dict):
        for key in ("version", "app_version", "package_version"):
            value = payload.get(key)
            if isinstance(value, str) and value.strip():
                return value.strip()
        for value in payload.values():
            found = _extract_version(value)
            if found:
                return found
    return None


def inspect_dev_cli(path: str, runner: Callable[[str, Sequence[str]], CommandResult] = run_command) -> DevCliCandidate:
    version_args = ["--version", "--json"]
    version_result = runner(path, version_args)
    version: str | None = None
    if version_result.returncode == 0:
        try:
            version = _extract_version(json.loads(version_result.stdout))
        except (json.JSONDecodeError, TypeError):
            version = None

    task_result = runner(path, ["task", "--help"])
    task_surface = task_result.returncode == 0
    if version is None or version in {"", "0.0.0", "unknown", "dev"}:
        detail = (
            "--version --json did not return a trustworthy version"
            if version_result.returncode == 0
            else f"--version --json exited {version_result.returncode}: "
            f"{(version_result.stderr or version_result.stdout).strip()}"
        )
        return DevCliCandidate(
            path, "version_unknown", None, version_args, task_surface, detail
        )
    if not task_surface:
        return DevCliCandidate(
            path,
            "surface_missing",
            version,
            version_args,
            False,
            "version is known but the task surface is unavailable",
        )
    return DevCliCandidate(
        path,
        "compatible_candidate",
        version,
        version_args,
        task_surface,
        "version and task surface inspected",
    )


def discover_dev_cli_candidates(explicit: str | None = None) -> list[str]:
    if explicit:
        return [explicit]
    candidates: list[str] = []
    for directory in os.get_exec_path():
        for name in ("simplicio-dev-cli", "simplicio-dev-cli.exe"):
            path = Path(directory) / name
            if path.is_file() and os.access(path, os.X_OK):
                resolved = str(path.resolve())
                if resolved not in candidates:
                    candidates.append(resolved)
    return candidates


def inspect_artifacts(root: Path) -> dict[str, object]:
    missing = [path for path in REQUIRED_ARTIFACTS if not (root / path).is_file()]
    return {
        "status": "ready" if not missing else "artifacts_missing",
        "required": list(REQUIRED_ARTIFACTS),
        "missing": missing,
    }


def inspect_runtime(
    root: Path,
    runtime: str | None,
    runner: Callable[[str, Sequence[str]], CommandResult] = run_command,
) -> dict[str, object]:
    artifacts = inspect_artifacts(root)
    if artifacts["missing"]:
        return {"status": "artifacts_missing", "artifacts": artifacts}
    path = runtime or os.environ.get("SIMPLICIO_RUNTIME_BIN") or shutil.which("simplicio")
    if not path:
        return {"status": "runtime_missing", "artifacts": artifacts, "path": None}
    result = runner(path, ["contracts", "smoke", "--json"])
    if result.returncode != 0:
        return {
            "status": "runtime_smoke_failed",
            "path": path,
            "artifacts": artifacts,
            "detail": (result.stderr or result.stdout).strip(),
        }
    try:
        smoke = json.loads(result.stdout)
    except json.JSONDecodeError:
        return {
            "status": "runtime_smoke_invalid",
            "path": path,
            "artifacts": artifacts,
            "detail": "contracts smoke returned non-JSON output",
        }
    if isinstance(smoke, dict) and str(smoke.get("status", "")).lower() in {"blocked", "failed", "error"}:
        status = "runtime_smoke_failed"
    else:
        status = "ready"
    return {"status": status, "path": path, "artifacts": artifacts, "smoke": smoke}


def preflight(
    root: Path,
    explicit_dev_cli: str | None = None,
    runtime: str | None = None,
    runner: Callable[[str, Sequence[str]], CommandResult] = run_command,
) -> dict[str, object]:
    paths = discover_dev_cli_candidates(explicit_dev_cli)
    candidates = [inspect_dev_cli(path, runner) for path in paths]
    selected = next((candidate for candidate in candidates if candidate.status == "compatible_candidate"), None)
    runtime_report = inspect_runtime(root, runtime, runner)
    status = "ready" if selected and runtime_report["status"] == "ready" else "blocked"
    return {
        "schema": "simplicio.code-preflight/v1",
        "status": status,
        "ok": status == "ready",
        "root": str(root),
        "selection": selected.to_dict() if selected else None,
        "dev_cli_candidates": [candidate.to_dict() for candidate in candidates],
        "runtime": runtime_report,
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Simplicio Code toolchain preflight")
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--dev-cli", help="Inspect only this explicit dev-cli path")
    parser.add_argument("--runtime", help="Inspect this explicit Runtime binary path")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    report = preflight(args.root.resolve(), args.dev_cli, args.runtime)
    output = json.dumps(report, indent=2, sort_keys=True) if args.json else json.dumps(report, indent=2)
    print(output)
    return 0 if report["ok"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
