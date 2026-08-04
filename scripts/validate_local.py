#!/usr/bin/env python3
"""Reproducible local validation lanes for Simplicio Code (#316).

The runner is intentionally small: it orchestrates existing repository tools,
never downloads dependencies, and records unavailable work as NOT_EXECUTED.
The HBP-like receipt is the canonical artifact; JSON is an explicit export.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import subprocess
import sys
import time
from dataclasses import dataclass
from pathlib import Path


SCHEMA = "simplicio.code-validation/v1"
PROFILES = ("fast", "deep", "release")


@dataclass
class Gate:
    name: str
    command: list[str]
    status: str = "NOT_EXECUTED"
    exit_code: int | None = None
    duration_ms: int = 0
    output: str = ""

    def run(self, cwd: Path, timeout: int) -> None:
        started = time.monotonic()
        try:
            completed = subprocess.run(
                self.command, cwd=cwd, capture_output=True, text=True,
                timeout=timeout, check=False,
            )
        except subprocess.TimeoutExpired as exc:
            self.status, self.exit_code = "FAIL", 124
            self.output = _redact((exc.stdout or "") + (exc.stderr or ""))[-4000:]
        except OSError as exc:
            self.status, self.exit_code = "NOT_EXECUTED", 127
            self.output = _redact(str(exc))
        else:
            self.exit_code = completed.returncode
            self.status = "PASS" if completed.returncode == 0 else "FAIL"
            self.output = _redact((completed.stdout + completed.stderr))[-4000:]
        self.duration_ms = round((time.monotonic() - started) * 1000)

    def as_dict(self) -> dict[str, object]:
        return {
            "name": self.name, "command": self.command, "status": self.status,
            "exit_code": self.exit_code, "duration_ms": self.duration_ms,
            "output_tail": self.output,
        }


def _redact(value: str) -> str:
    for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY", "SIMPLICIO_TOKEN", "GITHUB_TOKEN"):
        secret = os.environ.get(key)
        if secret:
            value = value.replace(secret, "[REDACTED]")
    return value


def _cmd(*args: str) -> list[str]:
    return [sys.executable, *args]


def gates(profile: str) -> list[Gate]:
    fast = [
        Gate("preflight", _cmd("scripts/preflight.py", "--root", ".", "--json")),
        Gate("workspace_access_audit", _cmd("scripts/audit_workspace_access.py", "--root", ".")),
        Gate("status_version_drift", _cmd("scripts/status_snapshot.py", "--root", ".", "--check")),
        Gate("python_compile", _cmd("-m", "compileall", "-q", "scripts", "python")),
        Gate("release_workflow_contract", _cmd("-m", "pytest", "scripts/tests/test_release_workflow.py", "-q")),
        Gate("deterministic_invariants", _cmd("scripts/check_deterministic_invariants.py", "--root", ".", "--json")),
        Gate("rust_toolchain", ["rustup", "toolchain", "list"]),
        Gate("rustfmt", ["cargo", "fmt", "--all", "--", "--check"]),
        Gate("headless_invocation_matrix", _cmd("scripts/run_headless_gate.py", "--root", ".")),
    ]
    deep = fast + [
        Gate("cargo_check", ["cargo", "check", "--workspace"]),
        Gate("cargo_test", ["cargo", "test", "--workspace"]),
    ]
    release = deep + [
        Gate("release_build", ["cargo", "build", "-p", "xai-grok-pager-bin", "--bin", "simplicio-code", "--profile", "release-dist"]),
    ]
    return {"fast": fast, "deep": deep, "release": release}[profile]


def run(profile: str, root: Path, output_dir: Path, timeout: int) -> dict[str, object]:
    output_dir.mkdir(parents=True, exist_ok=True)
    commit = subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True, check=False)
    dirty = subprocess.run(["git", "status", "--porcelain"], cwd=root, capture_output=True, text=True, check=False)
    report: dict[str, object] = {
        "schema": SCHEMA, "profile": profile, "root": str(root),
        "commit": commit.stdout.strip() or None, "dirty": bool(dirty.stdout.strip()),
        "environment": {"os": platform.platform(), "python": platform.python_version(), "arch": platform.machine()},
        "gates": [],
    }
    prior_failed = False
    for gate in gates(profile):
        if prior_failed:
            gate.output = "blocked by a previous gate"
        else:
            gate.run(root, timeout)
            prior_failed = gate.status != "PASS"
        report["gates"].append(gate.as_dict())
    statuses = [str(g["status"]) for g in report["gates"]]  # type: ignore[index]
    report["status"] = "PASS" if statuses and all(s == "PASS" for s in statuses) else "FAIL"
    report["receipt_sha256"] = hashlib.sha256(_canonical(report).encode()).hexdigest()
    _write_hbp(output_dir / "validation-receipt.hbp", report)
    (output_dir / "validation-summary.md").write_text(_summary(report), encoding="utf-8")
    return report


def _canonical(report: dict[str, object]) -> str:
    return json.dumps(report, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def _write_hbp(path: Path, report: dict[str, object]) -> None:
    lines = [f"schema={report['schema']}", f"profile={report['profile']}", f"status={report['status']}", f"commit={report['commit']}", f"receipt_sha256={report['receipt_sha256']}"]
    for gate in report["gates"]:  # type: ignore[union-attr]
        lines.append("gate=" + json.dumps(gate, sort_keys=True, ensure_ascii=False))
    path.write_text("\n".join(lines) + "\n", encoding="utf-8")


def _summary(report: dict[str, object]) -> str:
    lines = [f"# Local validation ({report['profile']})", "", f"- Status: `{report['status']}`", f"- Commit: `{report['commit']}`", f"- Dirty: `{report['dirty']}`", "", "| Gate | Status | Exit | Duration |", "| --- | --- | ---: | ---: |"]
    for gate in report["gates"]:  # type: ignore[union-attr]
        lines.append(f"| {gate['name']} | {gate['status']} | {gate['exit_code']} | {gate['duration_ms']} ms |")
    lines += ["", "A gate is `NOT_EXECUTED` when a prerequisite failed or a required local tool is unavailable; it is never treated as pass."]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=PROFILES, default="fast")
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output-dir", type=Path, default=Path("dist/local-validation"))
    parser.add_argument("--timeout", type=int, default=900)
    parser.add_argument("--json-export", type=Path)
    args = parser.parse_args(argv)
    report = run(args.profile, args.root.resolve(), args.output_dir.resolve(), args.timeout)
    if args.json_export:
        args.json_export.parent.mkdir(parents=True, exist_ok=True)
        args.json_export.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps({"schema": SCHEMA, "profile": args.profile, "status": report["status"], "receipt_sha256": report["receipt_sha256"]}, sort_keys=True))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
