#!/usr/bin/env python3
"""Run a command only after a disk-budget preflight (#52).

The wrapper is intentionally command-agnostic so the same gate can protect a
Loop plan, a Cargo build, or a test invocation. It writes a receipt containing
the initial and final disk observations and never performs cleanup itself.
"""
from __future__ import annotations

import argparse
import os
import subprocess
import sys
from datetime import datetime, timezone
from pathlib import Path

SCRIPT_DIR = Path(__file__).resolve().parent
sys.path.insert(0, str(SCRIPT_DIR))
import check_disk_budget as disk  # noqa: E402
from hbp_receipt import write_atomic  # noqa: E402


def utc_now() -> str:
    return datetime.now(timezone.utc).isoformat()


def receipt_payload(receipt: dict[str, object]) -> bytes:
    """Render the bounded receipt domain as deterministic UTF-8 HBP payload."""
    command = receipt["command"]
    initial = receipt["initial"]
    final = receipt["final"]
    assert isinstance(command, list) and isinstance(initial, dict) and isinstance(final, dict)
    fields = {
        "schema": receipt["schema"],
        "command": "\x1f".join(str(value) for value in command),
        "root": receipt["root"],
        "started_at": receipt["started_at"],
        "completed_at": receipt["completed_at"],
        "decision": receipt["decision"],
        "exit_code": receipt["exit_code"],
        "initial_free_bytes": initial["free_bytes"],
        "initial_required_bytes": initial["required_bytes"],
        "initial_status": initial["status"],
        "final_free_bytes": final["free_bytes"],
        "final_required_bytes": final["required_bytes"],
        "final_status": final["status"],
    }
    return "".join(f"{key}={fields[key]}\n" for key in sorted(fields)).encode("utf-8")


def write_receipt(path: Path, receipt: dict[str, object]) -> None:
    write_atomic(path, receipt_payload(receipt))


def run(command: list[str], root: str, min_free_bytes: int, receipt_path: Path) -> int:
    started = utc_now()
    initial = disk.run_preflight(root, min_free_bytes=min_free_bytes)
    receipt: dict[str, object] = {
        "schema": "simplicio.disk-preflight/v1",
        "command": command,
        "root": root,
        "started_at": started,
        "initial": initial.to_dict(),
        "decision": "blocked" if not initial.ok else "allowed",
    }
    if not initial.ok:
        receipt["completed_at"] = utc_now()
        receipt["exit_code"] = 2
        receipt["final"] = initial.to_dict()
        write_receipt(receipt_path, receipt)
        return 2

    completed = subprocess.run(command, cwd=root, check=False)
    final = disk.run_preflight(root, min_free_bytes=min_free_bytes)
    receipt.update(
        {
            "completed_at": utc_now(),
            "exit_code": completed.returncode,
            "final": final.to_dict(),
            "decision": "completed" if final.ok else "completed_disk_budget_breached",
        }
    )
    write_receipt(receipt_path, receipt)
    return completed.returncode if completed.returncode != 0 else (0 if final.ok else 2)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Run a command behind the disk-budget gate")
    parser.add_argument("--root", default=os.getcwd())
    parser.add_argument("--min-free-gb", type=float, default=disk.DEFAULT_MIN_FREE_BYTES / disk.GIB)
    parser.add_argument("--receipt", type=Path, required=True)
    parser.add_argument("command", nargs=argparse.REMAINDER, help="command after --")
    args = parser.parse_args(argv)
    if args.min_free_gb <= 0:
        parser.error("--min-free-gb must be positive")
    command = args.command
    if command and command[0] == "--":
        command = command[1:]
    if not command:
        parser.error("a command is required after --")
    return run(command, os.path.abspath(args.root), int(args.min_free_gb * disk.GIB), args.receipt)


if __name__ == "__main__":
    raise SystemExit(main())
