#!/usr/bin/env python3
"""Produce a fail-closed local recovery evidence bundle for issue #315."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import sys
import time
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Sequence


SCHEMA = "simplicio.code-recovery-evidence/v1"
MAX_OUTPUT_CHARS = 4000
TOOL_SPECS = (
    ("runtime", "SIMPLICIO_RUNTIME_BIN", ("simplicio",), ("version", "--json")),
    ("mapper", "SIMPLICIO_MAPPER_BIN", ("simplicio-mapper",), ("--version", "--json")),
    ("fast", "SIMPLICIO_FAST_BIN", ("simplicio-fast",), ("--version", "--json")),
    (
        "dev-cli",
        "SIMPLICIO_DEV_CLI_BIN",
        ("simplicio-dev-cli", "simplicio-py"),
        ("--version", "--json"),
    ),
    ("loop", "SIMPLICIO_LOOP_BIN", ("simplicio-loop",), ("--version", "--json")),
)
_SECRET_SHAPED = re.compile(
    r"(?P<name>token|secret|password|authorization|cookie|api[_-]?key)"
    r"(?P<separator>\s*[=:]\s*)"
    r"(?P<value>\"[^\"]*\"|'[^']*'|[^\s,;]+)",
    re.IGNORECASE,
)
_VERSION = re.compile(
    r"(?<!\d)(\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?)(?!\d)"
)


@dataclass(frozen=True)
class CommandResult:
    status: str
    exit_code: int | None
    duration_ms: int
    output_tail: str
    raw_output: str = ""


Runner = Callable[[Sequence[str], Path, float], CommandResult]


def _canonical(value: object) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def _redact(value: str, tail: bool = True) -> str:
    for key in (
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "SIMPLICIO_TOKEN",
        "GITHUB_TOKEN",
    ):
        secret = os.environ.get(key)
        if secret:
            value = value.replace(secret, "[REDACTED]")
    value = _SECRET_SHAPED.sub(
        lambda match: (
            f"{match.group('name')}{match.group('separator')}[REDACTED]"
        ),
        value,
    )
    return value[-MAX_OUTPUT_CHARS:] if tail else value


def _text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, bytes):
        return value.decode("utf-8", errors="replace")
    return str(value)


def run_command(
    argv: Sequence[str], cwd: Path, timeout: float
) -> CommandResult:
    started = time.monotonic()
    try:
        completed = subprocess.run(
            list(argv),
            cwd=cwd,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
            text=True,
            timeout=max(0.01, timeout),
            check=False,
            close_fds=False if os.name == "nt" else True,
        )
    except FileNotFoundError:
        return CommandResult("UNAVAILABLE", 127, _elapsed_ms(started), "executable not found")
    except subprocess.TimeoutExpired as exc:
        raw_output = _redact(
            _text(exc.stdout) + _text(exc.stderr), tail=False
        )
        return CommandResult(
            "TIMEOUT",
            124,
            _elapsed_ms(started),
            f"timeout after {timeout:g}s\n{raw_output[-MAX_OUTPUT_CHARS:]}",
            raw_output,
        )
    except OSError as exc:
        return CommandResult("FAIL", 127, _elapsed_ms(started), _redact(str(exc)))
    raw_output = _redact(completed.stdout or "", tail=False)
    status = "PASS" if completed.returncode == 0 else "FAIL"
    return CommandResult(
        status,
        completed.returncode,
        _elapsed_ms(started),
        raw_output[-MAX_OUTPUT_CHARS:],
        raw_output,
    )


def _elapsed_ms(started: float) -> int:
    return round((time.monotonic() - started) * 1000)


def _extract_version(output: str, tool_name: str | None = None) -> str | None:
    if tool_name == "runtime":
        try:
            payload = json.loads(output)
        except json.JSONDecodeError:
            payload = None
        if isinstance(payload, dict):
            runtime = payload.get("runtime")
            if isinstance(runtime, dict):
                candidate = runtime.get("version")
                if isinstance(candidate, str) and _VERSION.fullmatch(candidate):
                    return candidate
    match = _VERSION.search(output)
    return match.group(1) if match else None


def _resolve_tool(
    explicit: str | None, env_name: str, names: Sequence[str]
) -> str | None:
    if explicit:
        return explicit
    from_environment = os.environ.get(env_name)
    if from_environment:
        return from_environment
    for name in names:
        resolved = shutil.which(name)
        if resolved:
            return resolved
    return None


def probe_tool(
    name: str,
    path: str | None,
    version_args: Sequence[str],
    root: Path,
    timeout: float,
    runner: Runner,
) -> dict[str, object]:
    if not path:
        return {
            "name": name,
            "path": None,
            "command": [],
            "status": "UNAVAILABLE",
            "exit_code": 127,
            "duration_ms": 0,
            "version": None,
            "output_tail": "no executable found",
        }
    command = [path, *version_args]
    result = runner(command, root, timeout)
    version = _extract_version(
        result.raw_output or result.output_tail, name
    ) if result.status == "PASS" else None
    status = result.status
    output = result.output_tail
    if status == "PASS" and not version:
        status = "FAIL"
        output = "version command returned no trustworthy semver\n" + output
    return {
        "name": name,
        "path": path,
        "command": command,
        "status": status,
        "exit_code": result.exit_code,
        "duration_ms": result.duration_ms,
        "version": version,
        "output_tail": output,
    }


def _probe_git(root: Path, timeout: float, runner: Runner) -> dict[str, object]:
    commit = runner(["git", "rev-parse", "HEAD"], root, timeout)
    state = runner(["git", "status", "--porcelain"], root, timeout)
    dirty: bool | None
    if state.status == "PASS":
        dirty = bool(state.output_tail.strip())
    else:
        dirty = None
    return {
        "commit": commit.output_tail.strip().splitlines()[0] if commit.status == "PASS" and commit.output_tail.strip() else None,
        "commit_status": commit.status,
        "dirty": dirty,
        "status_command_status": state.status,
        "status_output_tail": state.output_tail,
    }


def _verify_validation_receipt(path: Path, root: Path) -> dict[str, object]:
    if not path.is_file():
        return {"present": False, "verified": False, "path": str(path)}
    receipt = {
        "present": True,
        "verified": False,
        "path": str(path),
        "sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
    }
    try:
        root_text = str(root)
        if root_text not in sys.path:
            sys.path.insert(0, root_text)
        from scripts import validate_local

        report = validate_local.verify_hbp(path)
        receipt["verified"] = True
        receipt["receipt_sha256"] = report.get("receipt_sha256")
    except (OSError, ValueError, ImportError) as exc:
        receipt["error"] = _redact(str(exc))
    return receipt


def _run_validation(
    root: Path,
    output_dir: Path,
    profile: str,
    timeout: float,
    runner: Runner,
    enabled: bool,
) -> dict[str, object]:
    validation_dir = output_dir / "local-validation"
    if not enabled:
        return {
            "status": "UNVERIFIED",
            "reason": "validation skipped by operator",
            "command": [],
            "receipt": {"present": False, "verified": False},
        }
    command = [
        sys.executable,
        str(root / "scripts" / "validate_local.py"),
        "--profile",
        profile,
        "--root",
        str(root),
        "--output-dir",
        str(validation_dir),
        "--timeout",
        str(timeout),
    ]
    result = runner(command, root, max(timeout * 12, 120.0))
    receipt = _verify_validation_receipt(
        validation_dir / "validation-receipt.hbp", root
    )
    status = result.status
    if status == "PASS" and not receipt.get("verified"):
        status = "FAIL"
    return {
        "status": status,
        "command": command,
        "exit_code": result.exit_code,
        "duration_ms": result.duration_ms,
        "output_tail": result.output_tail,
        "receipt": receipt,
        "profile": profile,
    }


def _without_digest(report: dict[str, object]) -> dict[str, object]:
    payload = dict(report)
    payload.pop("evidence_sha256", None)
    return payload


def evidence_digest(report: dict[str, object]) -> str:
    return hashlib.sha256(
        _canonical(_without_digest(report)).encode("utf-8")
    ).hexdigest()


def verify_report(path: Path) -> dict[str, object]:
    report = json.loads(path.read_text(encoding="utf-8"))
    expected = report.get("evidence_sha256")
    if not isinstance(expected, str) or expected != evidence_digest(report):
        raise ValueError("recovery evidence digest mismatch")
    return report


def collect(
    root: Path,
    output_dir: Path,
    profile: str = "fast",
    timeout: float = 60.0,
    run_validation: bool = True,
    runner: Runner = run_command,
    tool_paths: dict[str, str | None] | None = None,
) -> dict[str, object]:
    root = root.resolve()
    output_dir = output_dir.resolve()
    tool_paths = tool_paths or {}
    before = _probe_git(root, timeout, runner)
    tools: list[dict[str, object]] = []
    for name, env_name, names, version_args in TOOL_SPECS:
        path = tool_paths.get(name)
        tools.append(
            probe_tool(
                name,
                _resolve_tool(path, env_name, names),
                version_args,
                root,
                timeout,
                runner,
            )
        )
    validation = _run_validation(
        root, output_dir, profile, timeout, runner, run_validation
    )
    after = _probe_git(root, timeout, runner)
    tool_statuses = [str(tool["status"]) for tool in tools]
    hard_fail = any(
        status in {"FAIL", "TIMEOUT"} for status in tool_statuses
    ) or validation["status"] in {"FAIL", "TIMEOUT"}
    unverified = any(
        status in {"UNAVAILABLE", "UNVERIFIED"} for status in tool_statuses
    ) or validation["status"] == "UNVERIFIED"
    clean = before["dirty"] is False and after["dirty"] is False
    stable_revision = (
        isinstance(before.get("commit"), str)
        and before.get("commit") == after.get("commit")
    )
    if not stable_revision:
        hard_fail = True
    if before["dirty"] is None or after["dirty"] is None:
        unverified = True
    if not clean:
        hard_fail = True
    status = "PASS"
    if hard_fail:
        status = "FAIL"
    elif unverified or not clean:
        status = "UNVERIFIED"
    report: dict[str, object] = {
        "schema": SCHEMA,
        "status": status,
        "generated_at_utc": datetime.now(timezone.utc).isoformat(),
        "root": str(root),
        "commit": after["commit"] or before["commit"],
        "environment": {
            "os": platform.platform(),
            "python": platform.python_version(),
            "arch": platform.machine(),
        },
        "workspace_before": before,
        "workspace_after": after,
        "tools": tools,
        "local_validation": validation,
        "boundaries": {
            "workspace_revision_stable": stable_revision,
            "network": "NOT_USED",
            "github_actions": "NOT_REQUIRED_FOR_LOCAL_LANE",
            "remote_ci": "UNVERIFIED",
            "secrets_persisted": False,
        },
    }
    report["evidence_sha256"] = evidence_digest(report)
    return report


def _summary(report: dict[str, object]) -> str:
    lines = [
        "# Local recovery evidence",
        "",
        f"- Status: {report['status']}",
        f"- Commit: {report['commit']}",
        f"- Evidence SHA-256: {report['evidence_sha256']}",
        "",
        "| Tool | Version | Status | Path |",
        "| --- | --- | --- | --- |",
    ]
    for tool in report["tools"]:
        lines.append(
            f"| {tool['name']} | {tool['version'] or '-'} | {tool['status']} | "
            f"{tool['path'] or '-'} |"
        )
    validation = report["local_validation"]
    lines.extend(
        [
            "",
            f"- Local validation ({validation.get('profile', '-')}) status: {validation['status']}",
            f"- Validation receipt verified: {validation.get('receipt', {}).get('verified', False)}",
            "",
            "This bundle is fail-closed: unavailable tools, failed or timed-out gates, dirty checkouts, and unverified receipts never produce PASS.",
            "The lane is local and does not claim GitHub Actions, remote CI, release, install, downgrade, or downstream proof.",
            "",
        ]
    )
    return "\n".join(lines)


def write_bundle(report: dict[str, object], output_dir: Path) -> dict[str, str]:
    output_dir.mkdir(parents=True, exist_ok=True)
    json_path = output_dir / "recovery-evidence.json"
    summary_path = output_dir / "recovery-summary.md"
    json_path.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    verify_report(json_path)
    summary_path.write_text(_summary(report), encoding="utf-8")
    return {"json": str(json_path), "summary": str(summary_path)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output-dir", type=Path, default=Path("dist/recovery-evidence"))
    parser.add_argument("--profile", choices=("fast", "deep", "release"), default="fast")
    parser.add_argument("--timeout", type=float, default=60.0)
    parser.add_argument("--skip-validation", action="store_true")
    parser.add_argument("--runtime")
    parser.add_argument("--mapper")
    parser.add_argument("--fast")
    parser.add_argument("--dev-cli")
    parser.add_argument("--loop")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    tool_paths = {
        "runtime": args.runtime,
        "mapper": args.mapper,
        "fast": args.fast,
        "dev-cli": args.dev_cli,
        "loop": args.loop,
    }
    report = collect(
        args.root,
        args.output_dir,
        profile=args.profile,
        timeout=args.timeout,
        run_validation=not args.skip_validation,
        tool_paths=tool_paths,
    )
    paths = write_bundle(report, args.output_dir.resolve())
    payload = {
        "schema": SCHEMA,
        "status": report["status"],
        "commit": report["commit"],
        "evidence_sha256": report["evidence_sha256"],
        "paths": paths,
    }
    print(json.dumps(payload, sort_keys=True))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
