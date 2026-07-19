#!/usr/bin/env python3
"""Contract validation tests for #53."""
from __future__ import annotations

import copy
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts"))

from validate_loop_contract import validate_contract  # noqa: E402


VALID = {
    "identity": {"system": "simplicio-code", "feature": "runtime", "type": "change", "title": "Add runtime"},
    "scenarios": [{"id": "SC01", "name": "happy path", "rule_ids": ["RN01"]}],
    "rules": [{"id": "RN01", "text": "must use Runtime"}],
    "warnings": ["NFR was inferred"],
    "errors": [],
}


def main() -> int:
    checks = []

    def check(label, condition):
        checks.append(bool(condition))
        print(f"  [{'ok' if condition else 'XX'}] {label}")

    valid = validate_contract(VALID)
    check("complete contract is valid", valid["valid"] and valid["scenario_count"] == 1)
    check("warnings remain visible", valid["warnings"] == ["NFR was inferred"])

    for label, mutate, expected in [
        ("parser errors", lambda c: c.update(errors=["parser failed"]), "parser failed"),
        ("empty identity", lambda c: c["identity"].update(title=""), "identity.title is empty"),
        ("empty scenarios", lambda c: c.update(scenarios=[]), "scenarios must be a non-empty array"),
        ("empty rules", lambda c: c.update(rules=[]), "rules must be a non-empty array"),
        ("duplicate rule ids", lambda c: c["rules"].append({"id": "RN01"}), "duplicate ids"),
        (
            "unresolved required question",
            lambda c: c.update(questions=[{"required": True, "question": "NFR?"}]),
            "required question",
        ),
    ]:
        candidate = copy.deepcopy(VALID)
        mutate(candidate)
        result = validate_contract(candidate)
        check(label + " blocks dispatch", not result["valid"] and any(expected in e for e in result["errors"]))

    print(f"selftest: {'PASS' if all(checks) else 'FAIL'} ({sum(checks)}/{len(checks)})")
    return 0 if all(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
