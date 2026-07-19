#!/usr/bin/env python3
"""Hermetic tests for #52's wrapper and disposable cleanup guard."""
from __future__ import annotations

import json
import os
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
        receipt = root / "receipts" / "run.json"
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
        report = json.loads(receipt.read_text(encoding="utf-8"))
        check("allowed command runs and writes receipt", exit_code == 2 and receipt.is_file())
        check("receipt contains initial and final observations", "initial" in report and "final" in report)
        check("post-command disk breach is visible", report["decision"] == "completed_disk_budget_breached")

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
