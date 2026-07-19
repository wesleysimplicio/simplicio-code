#!/usr/bin/env python3
"""Tests for the non-blocking invariant report and headless matrix."""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts"))

from check_unused_struct_locals import scan_source  # noqa: E402
from headless_invocation_matrix import build_cases  # noqa: E402


def main() -> int:
    checks = []

    def check(label, condition):
        checks.append(bool(condition))
        print(f"  [{'ok' if condition else 'XX'}] {label}")

    source = """
fn build() {
    let search_backend = make_backend();
    let spec = AgentRebuildSpec {
        agent: agent,
    };
    consume(search_backend);
}
"""
    findings = scan_source(source, "fixture.rs")
    check("struct omission shape is reported", len(findings) == 1 and findings[0]["variable"] == "search_backend")

    source_with_use = """
fn build() {
    let search_backend = make_backend();
    let spec = AgentRebuildSpec {
        search_backend,
    };
}
"""
    check("correctly wired field is not reported", not scan_source(source_with_use, "fixture.rs"))

    cases = build_cases()
    check("matrix covers all combinations", len(cases) == 8)
    check("matrix includes positional always-approve", any("positional-always-approve" in case.name for case in cases))
    check("matrix includes no-tty and tty", {case.tty for case in cases} == {False, True})

    print(f"selftest: {'PASS' if all(checks) else 'FAIL'} ({sum(checks)}/{len(checks)})")
    return 0 if all(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
