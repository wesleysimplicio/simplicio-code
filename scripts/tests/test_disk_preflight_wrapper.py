#!/usr/bin/env python3
"""Hermetic tests for #52's wrapper and disposable cleanup guard."""
from __future__ import annotations

import os
import struct
import sys
import tempfile
from pathlib import Path
from types import SimpleNamespace

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts"))

import check_disk_budget as disk  # noqa: E402
import cleanup_disposable  # noqa: E402
import run_with_disk_preflight as wrapper  # noqa: E402


def main() -> int:
    checks = []

    def check(label, condition):
        checks.append(bool(condition))
        print(f"  [{'ok' if condition else 'XX'}] {label}")

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        target = root / "target"
        target.mkdir()
        (target / "artifact").write_bytes(b"x" * 32)
        receipt = root / "receipts" / "run.hbp"
        original = wrapper.disk.run_preflight
        observations = iter(
            [
                disk.check_disk_budget(10, 5, str(root), []),
                disk.check_disk_budget(4, 5, str(root), []),
            ]
        )
        wrapper.disk.run_preflight = lambda *_args, **_kwargs: next(observations)
        try:
            exit_code = wrapper.run([sys.executable, "-c", "pass"], str(root), 5, receipt)
        finally:
            wrapper.disk.run_preflight = original
        ledger = receipt.read_bytes()
        body_length = struct.unpack_from("<I", ledger, 8)[0]
        body = ledger[12:12 + body_length]
        cursor = 16
        decoded = []
        for _ in range(3):
            length = struct.unpack_from("<I", body, cursor)[0]
            cursor += 4
            decoded.append(body[cursor:cursor + length].decode("utf-8"))
            cursor += length
        payload = bytes.fromhex(decoded[1]).decode("utf-8")
        check("allowed command runs and writes HBP receipt", exit_code == 2 and ledger[:8] == b"HBP1\x01\x00\x00\x00")
        check("HBP record has exact bounded length", len(ledger) == 12 + body_length)
        check("receipt contains initial and final observations", "initial_free_bytes=10" in payload and "final_free_bytes=4" in payload)
        check("post-command disk breach is visible", "decision=completed_disk_budget_breached" in payload)

        planned = cleanup_disposable.cleanup(root, delete=False, environ={})
        check("cleanup defaults to a plan", planned["status"] == "planned" and target.exists())
        blocked = cleanup_disposable.cleanup(
            root, delete=True, environ={"SIMPLICIO_ALLOW_DISPOSABLE_CLEANUP": "0"}
        )
        check("delete requires explicit environment confirmation", blocked["status"] == "blocked_confirmation")
        deleted = cleanup_disposable.cleanup(
            root, delete=True, environ={"SIMPLICIO_ALLOW_DISPOSABLE_CLEANUP": "1"}
        )
        check("confirmed cleanup touches only target", deleted["status"] == "deleted" and not target.exists())
        check("source workspace remains", root.exists() and receipt.exists())

    print(f"selftest: {'PASS' if all(checks) else 'FAIL'} ({sum(checks)}/{len(checks)})")
    return 0 if all(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
