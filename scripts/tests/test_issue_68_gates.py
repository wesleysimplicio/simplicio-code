#!/usr/bin/env python3
"""Tests for the non-blocking invariant report and headless matrix."""
from __future__ import annotations

import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts"))

from check_unused_struct_locals import candidate_key, evaluate, scan_source  # noqa: E402
from headless_invocation_matrix import FATAL_PROCESS_CODES, PERMISSION_MODES, _classify_returncode, build_cases, execute  # noqa: E402
from audit_workspace_access import audit  # noqa: E402
from check_deterministic_invariants import check as check_deterministic_invariants  # noqa: E402


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
    review = evaluate(findings, set(), False)
    check("heuristic report is explicitly review-only", review["status"] == "REVIEW_ONLY" and not review["blocking"])
    enforced = evaluate(findings, {candidate_key(findings[0])}, True)
    check("exact allowlist passes enforcement", enforced["status"] == "PASS" and enforced["unallowlisted_count"] == 0)
    stale = evaluate(findings, {candidate_key(findings[0]), ("fixture.rs", 99, 100, "removed")}, True)
    check("stale allowlist fails closed", stale["status"] == "FAIL" and stale["stale_allowlist_count"] == 1)

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
    check("matrix covers all combinations", len(cases) == 28)
    check("matrix case names are unique", len({case.name for case in cases}) == len(cases))
    check("matrix includes positional always-approve", any("positional-always-approve" in case.name for case in cases))
    check("matrix includes no-tty and tty", {case.tty for case in cases} == {False, True})
    check(
        "matrix includes every permission mode",
        all(any(mode in case.approval_args for case in cases) for mode in PERMISSION_MODES),
    )
    check("fatal Windows process code is classified as crash", 0xC00000FD in FATAL_PROCESS_CODES)
    check("non-zero exits are failed cells", _classify_returncode(1) == ("failed", "exit_nonzero"))
    tty_case = next(case for case in cases if case.tty)
    tty_result = execute(sys.executable, tty_case, 1)
    check("TTY cells cannot be reported as observed without a terminal", tty_result["outcome"] == "not_executed" and not tty_result["observed"])

    access = audit(REPO, REPO / "docs/contracts/workspace-access-manifest.json")
    check(
        "workspace audit does not leak cfg(test) context",
        access["status"] == "passed" and not access["violations"] and not access["unclassified"],
    )
    check("deterministic initializer invariants pass", not check_deterministic_invariants(REPO))

    print(f"selftest: {'PASS' if all(checks) else 'FAIL'} ({sum(checks)}/{len(checks)})")
    return 0 if all(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
