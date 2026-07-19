#!/usr/bin/env python3
"""Plan/check the headless permission invocation matrix (#68).

Without a model/provider this script is an offline matrix generator, suitable
for CI and code review. With ``--binary`` it executes each combination with a
bounded timeout and records whether the process terminates; the caller chooses
the provider/model environment explicitly.
"""
from __future__ import annotations

import argparse
import itertools
import json
import os
import subprocess
from dataclasses import asdict, dataclass


@dataclass(frozen=True)
class Case:
    name: str
    prompt_args: tuple[str, ...]
    approval_args: tuple[str, ...]
    tty: bool

    def to_dict(self) -> dict[str, object]:
        value = asdict(self)
        value["prompt_args"] = list(self.prompt_args)
        value["approval_args"] = list(self.approval_args)
        return value


def build_cases() -> list[Case]:
    cases = []
    for prompt, approval, tty in itertools.product(
        (("-p", "ping"), ("ping",)),
        (("--always-approve",), ("--permission-mode", "bypassPermissions")),
        (False, True),
    ):
        name = "-".join(["single" if prompt[0] == "-p" else "positional", approval[0].lstrip("-"), "tty" if tty else "no-tty"])
        cases.append(Case(name, prompt, approval, tty))
    return cases


def execute(binary: str, case: Case, timeout_seconds: float) -> dict[str, object]:
    command = [binary, *case.approval_args, "--output-format", "json", *case.prompt_args]
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout_seconds, check=False)
        return {"case": case.to_dict(), "command": command, "terminated": True, "returncode": completed.returncode}
    except subprocess.TimeoutExpired:
        return {"case": case.to_dict(), "command": command, "terminated": False, "returncode": 124}
    except OSError as exc:
        return {"case": case.to_dict(), "command": command, "terminated": True, "returncode": 127, "error": str(exc)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Headless permission invocation matrix")
    parser.add_argument("--binary", help="execute the matrix against this binary")
    parser.add_argument("--timeout-seconds", type=float, default=20.0)
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    cases = build_cases()
    results = [execute(args.binary, case, args.timeout_seconds) for case in cases] if args.binary else [
        {"case": case.to_dict(), "planned": True} for case in cases
    ]
    report = {
        "schema": "simplicio.headless-invocation-matrix/v1",
        "offline": args.binary is None,
        "case_count": len(results),
        "results": results,
        "all_terminated": all(result.get("terminated", True) for result in results),
    }
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else json.dumps(report, indent=2))
    return 0 if report["all_terminated"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
