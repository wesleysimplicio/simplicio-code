#!/usr/bin/env python3
"""Read-only, provider-free onboarding diagnostics for Simplicio Code."""
from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import time
from typing import Any

SCHEMA = "simplicio.onboarding-doctor/v1"
BUNDLE_SCHEMA = "simplicio.onboarding-bundle/v1"
COMPONENTS = {"code", "agent-host", "runtime", "loop-hub", "mapper", "dev-cli", "fixtures"}
PRODUCTIVE = {"agent-host", "runtime", "loop-hub"}
SECRET = re.compile(r"(?i)(token|secret|password|api[_-]?key)=([^\s]+)")
VERSION = re.compile(r"(\d+)\.(\d+)\.(\d+)(?:[-+][0-9A-Za-z.-]+)?")


def redact(value: str) -> str:
    return SECRET.sub(r"\1=[REDACTED]", value)


def load_manifest(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != BUNDLE_SCHEMA:
        raise ValueError(f"manifest schema must be {BUNDLE_SCHEMA}")
    components = data.get("components")
    if not isinstance(components, list) or {x.get("name") for x in components if isinstance(x, dict)} != COMPONENTS:
        raise ValueError("manifest must contain each onboarding component exactly once")
    for item in components:
        if not VERSION.fullmatch(str(item.get("version", ""))):
            raise ValueError(f"{item.get('name')} version is not pinned semver")
        if item.get("name") != "fixtures" and not re.fullmatch(r"[a-z0-9-]+", str(item.get("probe", ""))):
            raise ValueError(f"{item.get('name')} has an unsafe probe")
    return data


def _version(executable: str) -> tuple[str | None, str | None]:
    try:
        result = subprocess.run([executable, "--version"], text=True, capture_output=True, timeout=3, check=False)
        text = redact((result.stdout or result.stderr).strip())[:300]
        match = VERSION.search(text)
        return (match.group(0) if match else None, None if result.returncode == 0 else f"version probe exited {result.returncode}")
    except (OSError, subprocess.TimeoutExpired) as exc:
        return None, redact(str(exc))


def _component(item: dict[str, Any], root: Path) -> dict[str, Any]:
    name = item["name"]
    if name == "fixtures":
        target = root / item["path"]
        found, detected, source = target.is_file(), item["version"] if target.is_file() else None, str(target)
        probe_error = None
    else:
        executable = shutil.which(item["probe"])
        found, source = executable is not None, executable
        detected, probe_error = _version(executable) if executable else (None, None)
    health = "ready" if found and detected else "blocked"
    blocker = None if health == "ready" else (probe_error or f"{name} is not installed or has no parseable version")
    return {"name": name, "expected_version": item["version"], "detected_version": detected,
            "capabilities": item["capabilities"], "origin": item["origin"], "source": source,
            "health": health, "blocker": blocker}


def _socket_check(path: str | None) -> dict[str, Any]:
    if not path:
        return {"health": "blocked", "blocker": "SIMPLICIO_AGENT_SOCKET is not set", "path": None}
    target = Path(path).expanduser()
    try:
        mode = target.stat().st_mode
        ok = target.is_socket() and not bool(mode & 0o002)
    except OSError:
        ok = False
    return {"health": "ready" if ok else "blocked", "blocker": None if ok else "socket missing, non-socket, or world-writable", "path": str(target)}


def doctor(manifest_path: Path, root: Path, mode: str) -> dict[str, Any]:
    started = time.monotonic_ns()
    manifest = load_manifest(manifest_path)
    components = [_component(item, root) for item in manifest["components"]]
    git = shutil.which("git")
    gh = shutil.which("gh")
    checks = {
        "path": {"health": "ready" if git else "blocked", "blocker": None if git else "git is not on PATH", "git": git, "github_cli": gh},
        "github_auth": {"health": "unknown", "blocker": "run `gh auth status` explicitly; doctor never reads credentials" if gh else "GitHub CLI is not installed"},
        "agent_socket": _socket_check(os.environ.get("SIMPLICIO_AGENT_SOCKET")),
        "worktree": {"health": "ready" if (root / ".git").exists() else "blocked", "blocker": None if (root / ".git").exists() else "root is not a Git worktree", "root": str(root.resolve())},
        "quota": {"health": "ready", "free_bytes": shutil.disk_usage(root).free, "blocker": None},
    }
    ready_names = {x["name"] for x in components if x["health"] == "ready"}
    productive = PRODUCTIVE <= ready_names and checks["agent_socket"]["health"] == "ready"
    # protocol_only is diagnostic evidence only; it can never grant effect authority.
    selected_ready = mode == "protocol_only" or productive
    blocker = None if selected_ready else "productive requires compatible AgentHost, Runtime, Loop Hub, and a secure persistent socket"
    return {"schema": SCHEMA, "bundle_version": manifest["bundle_version"], "mode": mode,
            "status": "ready" if selected_ready else "blocked", "effect_authority": False,
            "productive_ready": productive, "blocker": blocker, "components": components, "checks": checks,
            "metrics": {"preflight_ns": time.monotonic_ns() - started, "external_calls": sum(x["source"] is not None for x in components if x["name"] != "fixtures")}}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--manifest", type=Path, default=Path("config/onboarding-bundle-v1.json"))
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--mode", choices=("protocol_only", "productive"), default="protocol_only")
    args = parser.parse_args(argv)
    try:
        report = doctor(args.manifest, args.root, args.mode)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        report = {"schema": SCHEMA, "status": "error", "effect_authority": False, "blocker": redact(str(exc))}
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["status"] == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
