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
COMPATIBLE_HEALTH = {"ready", "compatible"}
SECRET_KEYS = {
    "token", "accesstoken", "refreshtoken", "secret", "password", "apikey",
    "authorization", "cookie", "setcookie", "prompt", "content", "code", "sourcecode",
}
VERSION = re.compile(r"(\d+)\.(\d+)\.(\d+)(?:[-+][0-9A-Za-z.-]+)?")
PINNED_VERSION = re.compile(r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?")
ASSIGNMENT_SECRET = re.compile(
    r"(?i)(\b(?:token|secret|password|api[_-]?key|authorization|cookie|prompt|content|source[_-]?code)\b\s*[=:]\s*)((?!bearer\b)(\"[^\"]*\"|'[^']*'|[^\s,;}]+))"
)
BEARER_SECRET = re.compile(r"(?i)(\bbearer\s+)[A-Za-z0-9._~+/=-]+")


def _normalized_key(key: str) -> str:
    return re.sub(r"[^a-z0-9]", "", key.lower())


def redact_payload(value: Any) -> Any:
    """Return a JSON-compatible copy with secret and private-content fields removed."""
    if isinstance(value, dict):
        return {
            key: "[REDACTED]" if _normalized_key(key) in SECRET_KEYS else redact_payload(item)
            for key, item in value.items()
        }
    if isinstance(value, list):
        return [redact_payload(item) for item in value]
    return value


def _redact_assignment(match: re.Match[str]) -> str:
    raw = match.group(3)
    if raw[:1] in {"\"", "'"}:
        return f"{match.group(1)}{raw[0]}[REDACTED]{raw[-1]}"
    return f"{match.group(1)}[REDACTED]"


def redact(value: str) -> str:
    try:
        parsed = json.loads(value)
    except (TypeError, json.JSONDecodeError):
        redacted = ASSIGNMENT_SECRET.sub(_redact_assignment, value)
        return BEARER_SECRET.sub(r"\1[REDACTED]", redacted)
    return json.dumps(redact_payload(parsed), ensure_ascii=False, sort_keys=True)


def load_manifest(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(data, dict):
        raise ValueError("manifest must be a JSON object")
    if data.get("schema") != BUNDLE_SCHEMA:
        raise ValueError(f"manifest schema must be {BUNDLE_SCHEMA}")
    if not isinstance(data.get("bundle_version"), str) or not PINNED_VERSION.fullmatch(data["bundle_version"]):
        raise ValueError("bundle_version must be pinned semver")
    compatibility = data.get("compatibility")
    current_major = compatibility.get("current_major") if isinstance(compatibility, dict) else None
    accepted_majors = compatibility.get("accepted_majors") if isinstance(compatibility, dict) else None
    if (
        not isinstance(current_major, int)
        or current_major < 0
        or not isinstance(accepted_majors, list)
        or not accepted_majors
        or any(not isinstance(major, int) or major < 0 for major in accepted_majors)
        or current_major not in accepted_majors
    ):
        raise ValueError("manifest compatibility must declare accepted majors including current_major")
    components = data.get("components")
    if not isinstance(components, list):
        raise ValueError("manifest components must be a list")
    names: list[str] = []
    for index, item in enumerate(components):
        if not isinstance(item, dict):
            raise ValueError(f"components[{index}] must be an object")
        name = item.get("name")
        if name not in COMPONENTS:
            raise ValueError(f"components[{index}] has unknown name")
        if name in names:
            raise ValueError(f"duplicate component {name}")
        names.append(name)
    if set(names) != COMPONENTS:
        raise ValueError("manifest must contain each onboarding component exactly once")
    for item in components:
        if not isinstance(item.get("version"), str) or not PINNED_VERSION.fullmatch(item["version"]):
            raise ValueError(f"{item.get('name')} version is not pinned semver")
        if not isinstance(item.get("capabilities"), list) or not all(
            isinstance(capability, str) and capability for capability in item["capabilities"]
        ):
            raise ValueError(f"{item.get('name')} capabilities must be a non-empty string list")
        if not isinstance(item.get("origin"), str) or not item["origin"]:
            raise ValueError(f"{item.get('name')} origin is required")
        if item.get("name") != "fixtures" and not re.fullmatch(r"[a-z0-9-]+", str(item.get("probe", ""))):
            raise ValueError(f"{item.get('name')} has an unsafe probe")
        if item.get("name") == "fixtures":
            fixture_path = item.get("path")
            if not isinstance(fixture_path, str) or Path(fixture_path).is_absolute() or ".." in Path(fixture_path).parts:
                raise ValueError("fixtures path must be relative to the checkout")
    return data


def _version(executable: str) -> tuple[str | None, str | None]:
    try:
        command = [executable, "--version"]
        if os.name == "nt" and Path(executable).suffix.lower() == ".py":
            command = [sys.executable, executable, "--version"]
        result = subprocess.run(
            command,
            stdin=subprocess.DEVNULL,
            text=True,
            capture_output=True,
            close_fds=True,
            timeout=3,
            check=False,
        )
        text = redact((result.stdout or result.stderr).strip())[:300]
        match = VERSION.search(text)
        return (match.group(0) if match else None, None if result.returncode == 0 else f"version probe exited {result.returncode}")
    except (OSError, subprocess.TimeoutExpired) as exc:
        return None, redact(str(exc))


def _version_health(
    expected: str,
    detected: str | None,
    probe_error: str | None,
    found: bool,
    accepted_majors: set[int],
) -> tuple[str, str | None]:
    if not found:
        return "missing", "dependency is not installed"
    if probe_error:
        return "degraded", probe_error
    if detected is None:
        return "installed", "version probe returned no parseable semver"
    detected_match = VERSION.fullmatch(detected)
    if detected == expected:
        return "ready", None
    if detected_match and int(detected_match.group(1)) in accepted_majors:
        return "compatible", f"detected {detected}; pinned {expected}"
    return "degraded", f"detected {detected}; incompatible with pinned {expected}"


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
    health, blocker = _version_health(
        item["version"], detected, probe_error, found, set(item["accepted_majors"])
    )
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
    accepted_majors = manifest["compatibility"]["accepted_majors"]
    components = [
        _component({**item, "accepted_majors": accepted_majors}, root)
        for item in manifest["components"]
    ]
    git = shutil.which("git")
    gh = shutil.which("gh")
    checks = {
        "path": {"health": "ready" if git else "blocked", "blocker": None if git else "git is not on PATH", "git": git, "github_cli": gh},
        "github_auth": {"health": "unknown", "blocker": "run `gh auth status` explicitly; doctor never reads credentials" if gh else "GitHub CLI is not installed"},
        "agent_socket": _socket_check(os.environ.get("SIMPLICIO_AGENT_SOCKET")),
        "worktree": {"health": "ready" if (root / ".git").exists() else "blocked", "blocker": None if (root / ".git").exists() else "root is not a Git worktree", "root": str(root.resolve())},
        "quota": {"health": "ready", "free_bytes": shutil.disk_usage(root).free, "blocker": None},
    }
    compatible_names = {x["name"] for x in components if x["health"] in COMPATIBLE_HEALTH}
    productive = PRODUCTIVE <= compatible_names and checks["agent_socket"]["health"] == "ready"
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
    except (OSError, TypeError, ValueError, KeyError, json.JSONDecodeError) as exc:
        report = {"schema": SCHEMA, "status": "error", "effect_authority": False, "blocker": redact(str(exc))}
    print(json.dumps(report, indent=2, sort_keys=True))
    return 0 if report["status"] == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
