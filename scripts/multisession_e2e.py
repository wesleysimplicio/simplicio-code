#!/usr/bin/env python3
"""Classify externally-produced multi-session E2E/benchmark evidence.

This harness never starts an LLM, provider, Runtime, Loop Hub, or Orca.  The
already-authenticated invoking coordinator owns all decisions and supplies an
external trace.  This program only validates and summarizes that trace; the
final approval remains a separate coordinator action.
"""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import re
import sys
from pathlib import Path
from typing import Any

SCHEMA = "simplicio.code-multisession-e2e/v1"
SURFACES = ("tui", "headless", "acp", "workspace")
STATES = {"working", "waiting", "blocked", "done"}
METRICS = (
    "latency_ms", "tokens", "context_bytes", "cache_hits", "cost",
    "throughput", "cpu_seconds", "rss_bytes", "queue_wait_ms", "quality",
)
SECRET = re.compile(
    r"(?i)(authorization\s*[:=]|bearer\s+[a-z0-9._-]+|github_pat_|gh[pousr]_|"
    r"sk-[a-z0-9_-]{12,}|signed[_ -]?url|[?&](?:token|signature|x-amz-signature)=)"
)


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = max(0, math.ceil(fraction * len(ordered)) - 1)
    return ordered[index]


def _metric(samples: list[dict[str, Any]], name: str) -> dict[str, Any]:
    values = [sample[name] for sample in samples
              if isinstance(sample.get(name), (int, float))
              and not isinstance(sample.get(name), bool)]
    if not values:
        return {"p50": None, "p95": None, "samples": 0,
                "reason": "metric was not observed; no value was estimated"}
    return {"p50": percentile(values, .50), "p95": percentile(values, .95),
            "samples": len(values), "reason": None}


def _error(errors: list[str], condition: bool, message: str) -> None:
    if not condition:
        errors.append(message)


def classify(trace: dict[str, Any], raw: bytes) -> dict[str, Any]:
    """Return a fail-closed, deterministic approval-candidate receipt."""
    errors: list[str] = []
    blocked: list[str] = []
    if SECRET.search(raw.decode("utf-8", errors="replace")):
        errors.append("privacy scan found a credential or signed-URL pattern")

    authority = trace.get("authority", {})
    _error(errors, authority.get("decision_owner") == "external_invoking_llm",
           "decision owner must be external_invoking_llm")
    for key in ("internal_provider_started", "local_llm_started", "orca_opened"):
        _error(errors, authority.get(key) is False, f"authority.{key} must be false")

    dependencies = trace.get("dependencies", {})
    for name in ("agent_host", "runtime", "loop_hub", "mapper"):
        if dependencies.get(name) != "installed":
            blocked.append(f"{name} is not installed")
    if trace.get("credentials") != "available":
        blocked.append("external coordinator credentials are unavailable")

    sessions = trace.get("sessions", [])
    session_ids = [item.get("session_id") for item in sessions if isinstance(item, dict)]
    _error(errors, len(sessions) >= 20, "at least 20 sessions are required")
    _error(errors, len(session_ids) == len(set(session_ids)) and all(session_ids),
           "session identity must be present and unique")
    observed_states = {item.get("state") for item in sessions if isinstance(item, dict)}
    _error(errors, STATES <= observed_states, "working/waiting/blocked/done must all be observed")
    _error(errors, all(item.get("identity_preserved") is True for item in sessions),
           "every session must preserve identity")

    surface_runs = trace.get("surfaces", [])
    by_surface = {run.get("surface"): run for run in surface_runs if isinstance(run, dict)}
    for surface in SURFACES:
        run = by_surface.get(surface, {})
        _error(errors, run.get("semantic_hash") not in (None, ""), f"{surface} semantic hash missing")
        _error(errors, run.get("status") == "passed", f"{surface} did not pass")
    hashes = {run.get("semantic_hash") for run in by_surface.values()}
    _error(errors, len(hashes) == 1, "surface semantics differ")

    worktrees = trace.get("worktrees", {})
    _error(errors, len(set(worktrees.get("concurrent_paths", []))) >= 2,
           "two distinct concurrent worktrees are required")
    _error(errors, worktrees.get("collision_detected") is True and
           worktrees.get("overwrite_prevented") is True,
           "worktree collision must be detected without overwrite")

    recovery = trace.get("recovery", {})
    for key in ("restart", "reconnect", "cancel", "replay", "unknown_effect_reconciled"):
        _error(errors, recovery.get(key) is True, f"recovery.{key} was not proven")
    _error(errors, recovery.get("duplicate_effects") == 0, "effects were duplicated")

    governance = trace.get("governance", {})
    _error(errors, governance.get("implementer") and governance.get("reviewer") and
           governance.get("approver") and len({governance.get("implementer"),
           governance.get("reviewer"), governance.get("approver")}) == 3,
           "implementer, reviewer, and approver must be distinct")
    _error(errors, governance.get("prototype_first") is True,
           "Prototype-First evidence missing")
    _error(errors, governance.get("final_e2e_approved") is False,
           "final E2E approval must remain false for the coordinator")

    delivery = trace.get("delivery", {})
    _error(errors, delivery.get("confirmed_count") == 1,
           "exactly one delivery must be confirmed")
    _error(errors, delivery.get("remote_requery") is True and delivery.get("receipt_hash"),
           "remote re-query and final receipt hash are required")

    samples = trace.get("benchmark_samples", [])
    _error(errors, len(samples) >= 2, "benchmark requires repeated fixed-fixture samples")
    benchmark = {name: _metric(samples, name) for name in METRICS}
    missing_metrics = [name for name, metric in benchmark.items() if metric["samples"] == 0]

    if errors:
        status = "FAILED"
    elif blocked:
        status = "BLOCKED"
    elif missing_metrics:
        status = "UNVERIFIED"
    else:
        status = "READY_FOR_COORDINATOR_APPROVAL"
    return {
        "schema": SCHEMA, "status": status, "final_e2e_approved": False,
        "trace_sha256": hashlib.sha256(raw).hexdigest(), "errors": errors,
        "blocked_reasons": blocked, "missing_metrics": missing_metrics,
        "benchmark": benchmark,
        "limitations": ["This harness validates supplied evidence; it does not invoke an LLM or perform final approval."],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("trace", type=Path, help="external coordinator trace (JSON)")
    parser.add_argument("--output", type=Path, help="optional external CLI receipt")
    args = parser.parse_args(argv)
    try:
        raw = args.trace.read_bytes()
        value = json.loads(raw)
        if not isinstance(value, dict):
            raise ValueError("trace root must be an object")
        receipt = classify(value, raw)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"multisession E2E: invalid trace: {exc}", file=sys.stderr)
        return 2
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0 if receipt["status"] == "READY_FOR_COORDINATOR_APPROVAL" else 1


if __name__ == "__main__":
    raise SystemExit(main())
