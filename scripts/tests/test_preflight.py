#!/usr/bin/env python3
"""Hermetic tests for the Code toolchain preflight (#58)."""
from __future__ import annotations

import json
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts"))

import preflight  # noqa: E402


def fake_runner(responses):
    def run(path, args):
        return responses.get((path, tuple(args)), preflight.CommandResult(127, "", "not configured"))

    return run


def main() -> int:
    checks = []

    def check(label, condition):
        checks.append(bool(condition))
        print(f"  [{'ok' if condition else 'XX'}] {label}")

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        good = root / "good-cli"
        stale = root / "stale-cli"
        runtime = root / "runtime"
        for path in (good, stale, runtime):
            path.touch()
        for artifact in preflight.REQUIRED_ARTIFACTS:
            target = root / artifact
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text("fixture", encoding="utf-8")

        responses = {
            (str(good), ("--version", "--json")): preflight.CommandResult(0, json.dumps({"version": "0.16.1"})),
            (str(good), ("task", "--help")): preflight.CommandResult(0, "usage: task"),
            (str(stale), ("--version", "--json")): preflight.CommandResult(2, "", "cmd required"),
            (str(stale), ("task", "--help")): preflight.CommandResult(0, "usage: task"),
            (str(runtime), ("contracts", "smoke", "--json")): preflight.CommandResult(0, json.dumps({"status": "ready"})),
        }
        stale_report = preflight.inspect_dev_cli(str(stale), fake_runner(responses))
        check("stale install is version_unknown", stale_report.status == "version_unknown")
        check("unknown version is not rewritten as 0.0.0", stale_report.version is None)

        no_task = root / "no-task-cli"
        no_task.touch()
        responses[(str(no_task), ("--version", "--json"))] = preflight.CommandResult(
            0, json.dumps({"version": "0.16.1"})
        )
        responses[(str(no_task), ("task", "--help"))] = preflight.CommandResult(2, "", "unknown command")
        no_task_report = preflight.inspect_dev_cli(str(no_task), fake_runner(responses))
        check("known version without task surface is blocked", no_task_report.status == "surface_missing")

        report = preflight.preflight(
            root,
            explicit_dev_cli=str(good),
            runtime=str(runtime),
            runner=fake_runner(responses),
        )
        check("valid explicit chain is ready", report["status"] == "ready")
        check("selected path is reported", report["selection"]["path"] == str(good))
        check("runtime smoke is included", report["runtime"]["smoke"]["status"] == "ready")

        (root / preflight.REQUIRED_ARTIFACTS[0]).unlink()
        blocked = preflight.inspect_runtime(root, str(runtime), fake_runner(responses))
        check("missing artifacts block before runtime smoke", blocked["status"] == "artifacts_missing")

    print(f"selftest: {'PASS' if all(checks) else 'FAIL'} ({sum(checks)}/{len(checks)})")
    return 0 if all(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
