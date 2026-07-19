#!/usr/bin/env python3
"""Validate a Simplicio Loop plan contract before dispatch (#53).

This is intentionally independent of the Loop implementation: CI, Code, and
operators can use the same fail-closed validator at the boundary where a JSON
plan becomes executable. A contract with parser errors or missing identity,
scenarios, rules, stable IDs, or required answers is never considered valid.
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any


def _text(value: Any) -> str:
    return value.strip() if isinstance(value, str) else ""


def _items(value: Any) -> list[dict[str, Any]]:
    if not isinstance(value, list):
        return []
    return [item for item in value if isinstance(item, dict)]


def _check_unique_ids(items: list[dict[str, Any]], label: str, errors: list[str]) -> None:
    ids = [_text(item.get("id") or item.get("scenario_id") or item.get("rule_id")) for item in items]
    if any(not item_id for item_id in ids):
        errors.append(f"{label} must give every item a stable non-empty id")
    counts: dict[str, int] = {}
    for item_id in ids:
        if item_id:
            counts[item_id] = counts.get(item_id, 0) + 1
    duplicates = sorted(item_id for item_id, count in counts.items() if count > 1)
    if duplicates:
        errors.append(f"{label} has duplicate ids: {', '.join(duplicates)}")


def validate_contract(contract: dict[str, Any], mode: str = "plan") -> dict[str, Any]:
    errors: list[str] = []
    warnings = [str(item) for item in contract.get("warnings", [])] if isinstance(contract.get("warnings"), list) else []
    source_errors = contract.get("errors", [])
    if isinstance(source_errors, list):
        errors.extend(str(item) for item in source_errors if str(item).strip())
    elif source_errors:
        errors.append("contract.errors must be an array")

    identity = contract.get("identity")
    if not isinstance(identity, dict):
        errors.append("identity is missing")
    else:
        for field in ("system", "feature", "type", "title"):
            if not _text(identity.get(field)):
                errors.append(f"identity.{field} is empty")

    scenarios = _items(contract.get("scenarios"))
    if not scenarios:
        errors.append("scenarios must be a non-empty array")
    _check_unique_ids(scenarios, "scenarios", errors)

    rules = _items(contract.get("rules"))
    if not rules:
        errors.append("rules must be a non-empty array")
    _check_unique_ids(rules, "rules", errors)
    rule_ids = {_text(rule.get("id") or rule.get("rule_id")) for rule in rules}
    for index, scenario in enumerate(scenarios):
        references = scenario.get("rule_ids") or scenario.get("rules") or []
        if isinstance(references, list) and references and any(str(ref) not in rule_ids for ref in references):
            errors.append(f"scenarios[{index}] references an unknown rule id")

    questions = _items(contract.get("questions")) + _items(contract.get("unknown_questions"))
    for index, question in enumerate(questions):
        required = question.get("required", question.get("mandatory", False))
        answer = question.get("answer", question.get("resolution"))
        status = _text(question.get("status")).lower()
        if required and not _text(answer) and status not in {"resolved", "answered", "known"}:
            errors.append(f"required question {index} is unresolved")

    result = {
        "schema": "simplicio.loop-contract-validation/v1",
        "mode": mode,
        "valid": not errors,
        "errors": errors,
        "warnings": warnings,
        "identity": identity if isinstance(identity, dict) else {},
        "scenario_count": len(scenarios),
        "rule_count": len(rules),
    }
    return result


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Fail-closed validation for a Loop contract")
    parser.add_argument("contract", type=Path)
    parser.add_argument("--mode", choices=("plan", "run", "batch"), default="plan")
    parser.add_argument("--json", action="store_true", help="emit the validation receipt as JSON")
    args = parser.parse_args(argv)
    try:
        payload = json.loads(args.contract.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        result = {
            "schema": "simplicio.loop-contract-validation/v1",
            "mode": args.mode,
            "valid": False,
            "errors": [f"cannot read contract: {exc}"],
            "warnings": [],
        }
        print(json.dumps(result, indent=2, sort_keys=True))
        return 2
    if not isinstance(payload, dict):
        result = {
            "schema": "simplicio.loop-contract-validation/v1",
            "mode": args.mode,
            "valid": False,
            "errors": ["contract root must be an object"],
            "warnings": [],
        }
    else:
        result = validate_contract(payload, args.mode)
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["valid"] else 2


if __name__ == "__main__":
    raise SystemExit(main())
