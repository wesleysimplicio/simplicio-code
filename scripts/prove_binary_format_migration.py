#!/usr/bin/env python3
"""Prove Code-owned binary-format lane without internal JSON evidence (#100).

Runs the no-internal-JSON policy scanner on simplicio-code-formats and the
scoped JSON boundary inventory. Emits a Markdown summary (not JSON).
"""
from __future__ import annotations

import argparse
import pathlib
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parents[1]


def run(cmd: list[str]) -> tuple[int, str]:
    proc = subprocess.run(cmd, cwd=ROOT, capture_output=True, text=True, check=False)
    out = (proc.stdout or "") + (proc.stderr or "")
    return proc.returncode, out


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--markdown", action="store_true", help="print Markdown summary")
    args = parser.parse_args(argv)

    steps: list[tuple[str, int, str]] = []

    code, out = run(
        [
            sys.executable,
            str(ROOT / "tools" / "policy_scan.py"),
            "--repo",
            "crates/codegen/simplicio-code-formats",
            "--policy",
            "policy/no-internal-json.toml",
            "--mode",
            "strict",
        ]
    )
    steps.append(("policy_scan_code_formats", code, out))

    code, out = run(
        [
            sys.executable,
            str(ROOT / "scripts" / "check_json_boundaries.py"),
            "--mode",
            "strict",
            "--max-findings",
            "100",
            "--scope-file",
            "config/json-boundaries-binary-formats.txt",
        ]
    )
    steps.append(("json_boundaries_binary_lane", code, out))

    # Unit proof of HBI migration lives in the formats crate.
    code, out = run(
        [
            "cargo",
            "test",
            "-p",
            "simplicio-code-formats",
            "--test",
            "migration_e2e",
            "--",
            "--nocapture",
        ]
    )
    steps.append(("formats_migration_e2e", code, out))

    failed = [name for name, code, _ in steps if code != 0]
    if args.markdown or True:
        print("# Binary-format migration proof (#100)")
        print()
        for name, code, out in steps:
            status = "PASS" if code == 0 else "FAIL"
            print(f"- `{name}`: **{status}** (exit {code})")
            if code != 0:
                print()
                print("```")
                print(out[-2000:])
                print("```")
                print()
        print()
        if failed:
            print(f"Overall: **FAIL** ({len(failed)} step(s))")
            return 1
        print("Overall: **PASS** — Code-owned binary lane has no internal JSON and migration E2E is green.")
        print()
        print("Notes: repository-wide inherited JSON remains inventoried as migration_pending/exception;")
        print("this proof is the release-blocking Code-owned surface.")
    return 0 if not failed else 1


if __name__ == "__main__":
    raise SystemExit(main())
