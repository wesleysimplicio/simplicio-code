#!/usr/bin/env python3
"""Validate external evidence required to close Prototype-First dependency #116.

This validator does not manufacture an installed Runtime or Loop result.  It
combines their signed/hashed receipts, checks the required capability and E2E
state vocabulary, and emits one deterministic, redacted acceptance receipt.
Unknown measurements are represented as ``null`` with a reason.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import time
from pathlib import Path
from typing import Any

SCHEMA = "simplicio.prototype-acceptance-evidence/v1"
LOOP_SCHEMA = "simplicio.loop-prototype-capabilities/v1"
RUNTIME_SCHEMA = "simplicio.runtime-prototype-preflight/v1"
E2E_SCHEMA = "simplicio.prototype-product-e2e/v1"
REQUIRED_LOOP_STATES = {
    "prototype_required", "gallery", "compare", "revise", "reject",
    "accept", "stale", "build_authorized",
}
REQUIRED_RUNTIME_TOOLS = {
    "simplicio_prototype_artifact_write",
    "simplicio_prototype_artifact_read",
}
REQUIRED_AUDIT_EVENTS = {
    "prototype_published", "comparison_opened", "decision_rejected",
    "revision_requested", "decision_accepted", "build_authorized", "delivered",
}
REQUIRED_SURFACES = {"tui", "ui", "headless", "acp"}
REQUIRED_STEPS = {
    "install", "prototype", "compare", "reject", "revise", "accept",
    "build", "delivery",
}
HEX64 = re.compile(r"^[0-9a-f]{64}$")


def _safe(value: Any) -> bool:
    return isinstance(value, str) and bool(value) and not any(ord(c) < 32 for c in value)


def _load(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: receipt root must be an object")
    return value


def _digest(value: Any) -> str:
    encoded = json.dumps(
        value, sort_keys=True, separators=(",", ":"), ensure_ascii=True, allow_nan=False
    ).encode()
    return hashlib.sha256(encoded).hexdigest()


def _string_set(receipt: dict[str, Any], field: str, owner: str, errors: list[str]) -> set[str]:
    """Read a capability collection without letting malformed JSON crash the gate."""
    value = receipt.get(field)
    if not isinstance(value, list) or not all(_safe(item) for item in value):
        errors.append(f"{owner} {field} must be an array of non-empty strings")
        return set()
    if len(value) != len(set(value)):
        errors.append(f"{owner} {field} must not contain duplicates")
    return set(value)


def validate(loop: dict[str, Any], runtime: dict[str, Any], e2e: dict[str, Any]) -> dict[str, Any]:
    errors: list[str] = []
    if loop.get("schema") != LOOP_SCHEMA:
        errors.append(f"loop schema must be {LOOP_SCHEMA}")
    if runtime.get("schema") != RUNTIME_SCHEMA:
        errors.append(f"runtime schema must be {RUNTIME_SCHEMA}")
    if e2e.get("schema") != E2E_SCHEMA:
        errors.append(f"e2e schema must be {E2E_SCHEMA}")

    revisions = [loop.get("source_revision"), runtime.get("source_revision"), e2e.get("source_revision")]
    plans = [loop.get("plan_id"), runtime.get("plan_id"), e2e.get("plan_id")]
    if not all(_safe(item) for item in revisions) or len(set(revisions)) != 1:
        errors.append("source revision must be present and identical in all receipts")
    if not all(_safe(item) for item in plans) or len(set(plans)) != 1:
        errors.append("plan id must be present and identical in all receipts")

    loop_states = _string_set(loop, "states", "Loop", errors)
    missing_states = sorted(REQUIRED_LOOP_STATES - loop_states)
    if missing_states:
        errors.append("Loop capability handshake missing states: " + ", ".join(missing_states))
    if loop.get("accepted") is not True:
        errors.append("Loop did not accept the Code prototype contract")
    if loop.get("capability_issue") != 568:
        errors.append("Loop capability receipt must identify upstream issue 568")

    tools = _string_set(runtime, "tools", "Runtime", errors)
    missing_tools = sorted(REQUIRED_RUNTIME_TOOLS - tools)
    if missing_tools:
        errors.append("Runtime capability handshake missing tools: " + ", ".join(missing_tools))
    if runtime.get("negotiated") is not True or not _safe(runtime.get("binary_version")):
        errors.append("Runtime preflight did not negotiate a versioned real binary")
    if not HEX64.fullmatch(str(runtime.get("binary_sha256", ""))):
        errors.append("Runtime binary_sha256 must be a lowercase SHA-256 digest")
    if runtime.get("artifact_sanitization") != "passed":
        errors.append("Runtime artifact path/content sanitization evidence did not pass")
    if runtime.get("telemetry_emitted") is not False:
        errors.append("Runtime prototype operations must prove that no telemetry was emitted")

    runs_value = e2e.get("runs")
    runs = runs_value if isinstance(runs_value, list) else []
    if not isinstance(runs_value, list):
        errors.append("E2E runs must be an array")
    passed_surfaces: set[str] = set()
    seen_surfaces: set[str] = set()
    for index, run in enumerate(runs):
        if not isinstance(run, dict):
            errors.append(f"E2E run {index} must be an object")
            continue
        surface = run.get("surface")
        if surface not in REQUIRED_SURFACES:
            errors.append(f"E2E run {index} has an unknown surface")
            continue
        if surface in seen_surfaces:
            errors.append(f"E2E surface {surface} has contradictory or duplicate results")
        seen_surfaces.add(surface)
        steps = _string_set(run, "steps", f"E2E surface {surface}", errors)
        if run.get("status") not in {"passed", "failed"}:
            errors.append(f"E2E surface {surface} status must be passed or failed")
        if run.get("status") != "passed":
            continue
        if REQUIRED_STEPS <= steps:
            passed_surfaces.add(surface)
    missing_surfaces = sorted(REQUIRED_SURFACES - passed_surfaces)
    if missing_surfaces:
        errors.append("complete product E2E missing surfaces: " + ", ".join(missing_surfaces))
    if e2e.get("failure_injection_passed") is not True:
        errors.append("failure injection/cancellation/rollback evidence did not pass")
    if e2e.get("replay_hash_match") is not True:
        errors.append("deterministic replay hashes did not match")
    replay = e2e.get("replay_hashes")
    if (not isinstance(replay, list) or len(replay) < 2
            or not all(HEX64.fullmatch(str(item)) for item in replay)
            or len(set(replay)) != 1):
        errors.append("replay_hashes must contain at least two identical lowercase SHA-256 digests")
    audit_events = _string_set(e2e, "audit_events", "E2E", errors)
    missing_audit = sorted(REQUIRED_AUDIT_EVENTS - audit_events)
    if missing_audit:
        errors.append("E2E audit trail missing events: " + ", ".join(missing_audit))
    if e2e.get("audit_sanitized") is not True:
        errors.append("E2E audit records were not proven sanitized")
    if e2e.get("telemetry_emitted") is not False:
        errors.append("E2E prototype flow must prove that no telemetry was emitted")
    authorization = e2e.get("build_authorization_sha256")
    delivery = e2e.get("delivery_authorization_sha256")
    if not HEX64.fullmatch(str(authorization or "")) or delivery != authorization:
        errors.append("delivery must reference the exact Build authorization SHA-256")

    try:
        inputs: dict[str, str | None] = {
            "loop": _digest(loop), "runtime": _digest(runtime), "e2e": _digest(e2e)
        }
    except (TypeError, ValueError):
        errors.append("input receipts must contain canonical JSON values")
        inputs = {"loop": None, "runtime": None, "e2e": None}
    result: dict[str, Any] = {
        "schema": SCHEMA,
        "status": "ready" if not errors else "blocked",
        "source_revision": revisions[0] if revisions else None,
        "plan_id": plans[0] if plans else None,
        "input_sha256": inputs,
        "errors": errors,
        "metrics": {
            "validation_duration_ms": None,
            "reason": "wall-clock timing is intentionally excluded from deterministic receipts; use --benchmark",
        },
    }
    result["receipt_sha256"] = _digest(result)
    return result


def benchmark(loop: dict[str, Any], runtime: dict[str, Any], e2e: dict[str, Any], iterations: int) -> dict[str, Any]:
    start = time.perf_counter_ns()
    for _ in range(iterations):
        validate(loop, runtime, e2e)
    elapsed = time.perf_counter_ns() - start
    return {
        "iterations": iterations,
        "elapsed_ms": round(elapsed / 1_000_000, 3),
        "mean_us": round(elapsed / iterations / 1_000, 3),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--loop", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, required=True)
    parser.add_argument("--e2e", type=Path, required=True)
    parser.add_argument("--benchmark", type=int, metavar="ITERATIONS")
    args = parser.parse_args(argv)
    try:
        loop, runtime, e2e = _load(args.loop), _load(args.runtime), _load(args.e2e)
        result = validate(loop, runtime, e2e)
        if args.benchmark is not None:
            if args.benchmark < 1:
                raise ValueError("benchmark iterations must be positive")
            result["benchmark"] = benchmark(loop, runtime, e2e, args.benchmark)
    except (OSError, TypeError, ValueError, json.JSONDecodeError) as exc:
        result = {"schema": SCHEMA, "status": "blocked", "errors": [str(exc)]}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result["status"] == "ready" else 2


if __name__ == "__main__":
    raise SystemExit(main())
