#!/usr/bin/env python3
"""Reproducible local validation lanes for Simplicio Code (#316)."""
from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
import platform
import re
import signal
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


SCHEMA = "simplicio.code-validation/v1"
GATE_SCHEMA = "simplicio.code-validation-gate/v1"
HBP_SCHEMA = "simplicio.validation-receipt/v1"
PROFILES = ("fast", "deep", "release")
MAX_OUTPUT_CHARS = 4000
_SECRET_SHAPED = re.compile(
    r"(?P<name>token|secret|password|authorization|cookie|api[_-]?key)"
    r"(?P<separator>\s*[=:]\s*)"
    r"(?P<value>\"[^\"]*\"|'[^']*'|[^\s,;]+)",
    re.IGNORECASE,
)


@dataclass
class Gate:
    name: str
    command: list[str]
    status: str = "NOT_EXECUTED"
    exit_code: int | None = None
    duration_ms: int = 0
    output: str = ""
    artifact: str | None = None
    artifact_sha256: str | None = None

    def run(
        self,
        cwd: Path,
        timeout: float,
        cancel_event: threading.Event | None = None,
    ) -> None:
        started = time.monotonic()
        process: subprocess.Popen[bytes] | None = None
        with tempfile.TemporaryFile() as stream:
            try:
                process = subprocess.Popen(
                    self.command,
                    cwd=cwd,
                    stdin=subprocess.DEVNULL,
                    stdout=stream,
                    stderr=subprocess.STDOUT,
                    close_fds=False if os.name == "nt" else True,
                )
            except OSError as exc:
                self.status, self.exit_code = "NOT_EXECUTED", 127
                self.output = _redact(str(exc))
            else:
                deadline = started + max(0.01, timeout)
                while process.poll() is None:
                    if cancel_event is not None and cancel_event.is_set():
                        _stop_process(process)
                        self.status, self.exit_code = "CANCELLED", 130
                        self.output = _redact(
                            "cancelled by operator\n" + _tail(stream)
                        )
                        break
                    if time.monotonic() >= deadline:
                        _stop_process(process)
                        self.status, self.exit_code = "FAIL", 124
                        self.output = _redact(
                            f"timeout after {timeout:g}s\n" + _tail(stream)
                        )
                        break
                    time.sleep(0.05)
                else:
                    self.exit_code = process.returncode
                    self.status = "PASS" if process.returncode == 0 else "FAIL"
                    self.output = _redact(_tail(stream))
        self.duration_ms = round((time.monotonic() - started) * 1000)

    def as_dict(self) -> dict[str, object]:
        return {
            "name": self.name,
            "command": self.command,
            "status": self.status,
            "exit_code": self.exit_code,
            "duration_ms": self.duration_ms,
            "output_tail": self.output,
            "artifact": self.artifact,
            "artifact_sha256": self.artifact_sha256,
        }


def _redact(value: str) -> str:
    for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY", "SIMPLICIO_TOKEN", "GITHUB_TOKEN"):
        secret = os.environ.get(key)
        if secret:
            value = value.replace(secret, "[REDACTED]")
    value = _SECRET_SHAPED.sub(
        lambda match: (
            f"{match.group('name')}{match.group('separator')}[REDACTED]"
        ),
        value,
    )
    return value[-MAX_OUTPUT_CHARS:]


def _tail(stream: Any) -> str:
    stream.flush()
    stream.seek(0, os.SEEK_END)
    size = stream.tell()
    stream.seek(max(0, size - MAX_OUTPUT_CHARS))
    return stream.read().decode("utf-8", errors="replace")


def _stop_process(process: subprocess.Popen[bytes]) -> None:
    try:
        process.terminate()
        process.wait(timeout=1)
    except (OSError, subprocess.TimeoutExpired):
        try:
            process.kill()
            process.wait(timeout=1)
        except (OSError, subprocess.TimeoutExpired):
            pass


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
        Gate(
            "release_build",
            ["cargo", "build", "-p", "xai-grok-pager-bin", "--bin", "simplicio-code", "--profile", "release-dist"],
        ),
    ]
    return {"fast": fast, "deep": deep, "release": release}[profile]


def _safe_name(value: str) -> str:
    return re.sub(r"[^A-Za-z0-9_.-]+", "-", value).strip("-") or "gate"


def _write_gate_artifact(output_dir: Path, index: int, gate: Gate) -> None:
    payload = {
        "schema": GATE_SCHEMA,
        "name": gate.name,
        "command": gate.command,
        "status": gate.status,
        "exit_code": gate.exit_code,
        "duration_ms": gate.duration_ms,
        "output_tail": gate.output,
    }
    path = output_dir / f"gate-{index:02d}-{_safe_name(gate.name)}.json"
    path.write_text(json.dumps(payload, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    gate.artifact = path.name
    gate.artifact_sha256 = hashlib.sha256(path.read_bytes()).hexdigest()


def _without_digest(report: dict[str, object]) -> dict[str, object]:
    payload = dict(report)
    payload.pop("receipt_sha256", None)
    return payload


def _canonical(value: object) -> str:
    return json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":"))


def _receipt_digest(report: dict[str, object]) -> str:
    return hashlib.sha256(_canonical(_without_digest(report)).encode("utf-8")).hexdigest()


def _write_hbp(path: Path, report: dict[str, object]) -> None:
    encoded = base64.urlsafe_b64encode(_canonical(report).encode("utf-8")).decode("ascii")
    path.write_text(
        "\n".join(
            [
                f"schema={HBP_SCHEMA}",
                "format=simplicio.hbp/v1",
                f"receipt_sha256={report['receipt_sha256']}",
                f"report_b64={encoded}",
                "",
            ]
        ),
        encoding="utf-8",
    )


def read_hbp(path: Path | str) -> dict[str, object]:
    path = Path(path)
    fields: dict[str, str] = {}
    for line in path.read_text(encoding="utf-8").splitlines():
        key, separator, value = line.partition("=")
        if separator:
            fields[key] = value
    if fields.get("schema") != HBP_SCHEMA or fields.get("format") != "simplicio.hbp/v1":
        raise ValueError("invalid validation receipt header")
    encoded = fields.get("report_b64")
    if not encoded:
        raise ValueError("validation receipt has no report")
    try:
        report = json.loads(base64.urlsafe_b64decode(encoded).decode("utf-8"))
    except (ValueError, UnicodeDecodeError, json.JSONDecodeError) as exc:
        raise ValueError("validation receipt report is malformed") from exc
    if not isinstance(report, dict):
        raise ValueError("validation receipt report is not an object")
    return report


def verify_hbp(path: Path | str) -> dict[str, object]:
    path = Path(path)
    report = read_hbp(path)
    expected = report.get("receipt_sha256")
    if not isinstance(expected, str) or expected != _receipt_digest(report):
        raise ValueError("validation receipt digest mismatch")
    gates_report = report.get("gates")
    if not isinstance(gates_report, list):
        raise ValueError("validation receipt gates are malformed")
    for gate in gates_report:
        if not isinstance(gate, dict):
            raise ValueError("validation receipt gate is malformed")
        artifact = gate.get("artifact")
        artifact_sha = gate.get("artifact_sha256")
        if not isinstance(artifact, str) or Path(artifact).name != artifact:
            raise ValueError("validation receipt artifact path escapes output directory")
        if not isinstance(artifact_sha, str):
            raise ValueError("validation receipt artifact hash is missing")
        artifact_path = path.parent / artifact
        if not artifact_path.is_file():
            raise ValueError(f"validation artifact is missing: {artifact}")
        actual = hashlib.sha256(artifact_path.read_bytes()).hexdigest()
        if actual != artifact_sha:
            raise ValueError(f"validation artifact digest mismatch: {artifact}")
    return report


def run(
    profile: str,
    root: Path,
    output_dir: Path,
    timeout: float,
    cancel_event: threading.Event | None = None,
) -> dict[str, object]:
    output_dir.mkdir(parents=True, exist_ok=True)
    commit = subprocess.run(
        ["git", "rev-parse", "HEAD"], cwd=root, stdin=subprocess.DEVNULL, capture_output=True, text=True, check=False, close_fds=False if os.name == "nt" else True
    )
    dirty = subprocess.run(
        ["git", "status", "--porcelain"], cwd=root, stdin=subprocess.DEVNULL, capture_output=True, text=True, check=False, close_fds=False if os.name == "nt" else True
    )
    report: dict[str, object] = {
        "schema": SCHEMA,
        "profile": profile,
        "root": str(root),
        "commit": commit.stdout.strip() or None,
        "dirty": bool(dirty.stdout.strip()),
        "environment": {
            "os": platform.platform(),
            "python": platform.python_version(),
            "arch": platform.machine(),
        },
        "gates": [],
    }
    prior_failed = False
    for index, gate in enumerate(gates(profile), start=1):
        if prior_failed:
            gate.output = "blocked by a previous gate; job was not started"
        elif cancel_event is not None and cancel_event.is_set():
            gate.status, gate.exit_code, gate.output = "CANCELLED", 130, "cancelled by operator"
        else:
            gate.run(root, timeout, cancel_event)
        _write_gate_artifact(output_dir, index, gate)
        report["gates"].append(gate.as_dict())
        prior_failed = prior_failed or gate.status != "PASS"
    statuses = [str(g["status"]) for g in report["gates"]]  # type: ignore[index]
    if any(status == "CANCELLED" for status in statuses):
        report["status"] = "CANCELLED"
    else:
        report["status"] = "PASS" if statuses and all(status == "PASS" for status in statuses) else "FAIL"
    report["receipt_sha256"] = _receipt_digest(report)
    receipt_path = output_dir / "validation-receipt.hbp"
    _write_hbp(receipt_path, report)
    verify_hbp(receipt_path)
    (output_dir / "validation-summary.md").write_text(_summary(report), encoding="utf-8")
    return report


def _summary(report: dict[str, object]) -> str:
    lines = [
        f"# Local validation ({report['profile']})",
        "",
        f"- Status: `{report['status']}`",
        f"- Commit: `{report['commit']}`",
        f"- Dirty: `{report['dirty']}`",
        f"- Receipt: `validation-receipt.hbp` ({report['receipt_sha256']})",
        "",
        "| Gate | Status | Exit | Duration | Artifact |",
        "| --- | --- | ---: | ---: | --- |",
    ]
    for gate in report["gates"]:  # type: ignore[union-attr]
        lines.append(
            f"| {gate['name']} | {gate['status']} | {gate['exit_code']} | "
            f"{gate['duration_ms']} ms | `{gate['artifact']}` |"
        )
    lines += [
        "",
        "A gate is `NOT_EXECUTED` when a prerequisite failed or a required local tool is unavailable; it is never treated as pass.",
        "A `CANCELLED` run is not a pass. Every gate artifact is hashed into the HBP receipt and verified before the command returns.",
    ]
    return "\n".join(lines) + "\n"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--profile", choices=PROFILES, default="fast")
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output-dir", type=Path, default=Path("dist/local-validation"))
    parser.add_argument("--timeout", type=float, default=900)
    parser.add_argument("--json-export", type=Path)
    args = parser.parse_args(argv)
    cancel_event = threading.Event()
    previous_handler = signal.getsignal(signal.SIGINT)

    def request_cancel(_signum: int, _frame: Any) -> None:
        cancel_event.set()

    try:
        signal.signal(signal.SIGINT, request_cancel)
        report = run(args.profile, args.root.resolve(), args.output_dir.resolve(), args.timeout, cancel_event)
    finally:
        signal.signal(signal.SIGINT, previous_handler)
    if args.json_export:
        args.json_export.parent.mkdir(parents=True, exist_ok=True)
        args.json_export.write_text(json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(
        json.dumps(
            {
                "schema": SCHEMA,
                "profile": args.profile,
                "status": report["status"],
                "receipt_sha256": report["receipt_sha256"],
            },
            sort_keys=True,
        )
    )
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())