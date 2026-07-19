#!/usr/bin/env python3
"""Validate the neutral Code↔coordinator session contract."""

from __future__ import annotations

import argparse
import json
from typing import Any

SCHEMA = "simplicio.coordinator-protocol/v1"
COORDINATORS = {"builtin", "simplicio-agent", "external"}
TRANSITIONS = {
    "session.open": {"idle"},
    "turn.start": {"ready", "cancelled", "completed"},
    "turn.cancel": {"running", "awaiting_approval"},
    "approval.resolve": {"awaiting_approval"},
    "turn.resume": {"cancelled", "disconnected"},
    "session.close": {"ready", "cancelled", "completed", "disconnected"},
}


def validate(envelope: dict[str, Any]) -> dict[str, Any]:
    errors: list[str] = []
    if envelope.get("schema") != SCHEMA:
        errors.append("schema must be simplicio.coordinator-protocol/v1")
    coordinator = envelope.get("coordinator")
    if coordinator not in COORDINATORS:
        errors.append("coordinator must be builtin, simplicio-agent, or external")
    for field in ("workspace_id", "session_id", "turn_id", "policy_revision"):
        if not isinstance(envelope.get(field), str) or not envelope[field].strip():
            errors.append(f"{field} is required")
    events = envelope.get("events", [])
    if not isinstance(events, list):
        errors.append("events must be a list")
        events = []
    state = "idle"
    for index, event in enumerate(events):
        if not isinstance(event, dict):
            errors.append(f"events[{index}] must be an object")
            continue
        name = event.get("type")
        if name not in TRANSITIONS:
            errors.append(f"events[{index}] has unknown type {name!r}")
            continue
        if state not in TRANSITIONS[name]:
            errors.append(f"events[{index}] {name} is invalid from state {state}")
        for field in ("sequence", "causal_id"):
            if field not in event:
                errors.append(f"events[{index}] missing {field}")
        if name == "session.open":
            state = "ready"
        elif name == "turn.start":
            state = "running"
        elif name == "turn.cancel":
            state = "cancelled"
        elif name == "approval.resolve":
            state = "running"
        elif name == "turn.resume":
            state = "running"
        elif name == "session.close":
            state = "closed"
    sequences = [event.get("sequence") for event in events if isinstance(event, dict)]
    if sequences != sorted(set(sequences)):
        errors.append("event sequence values must be unique and strictly increasing")
    return {"schema": SCHEMA, "status": "blocked" if errors else "ready", "state": state, "errors": errors}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("input", nargs="?", default="-")
    args = parser.parse_args(argv)
    raw = __import__("sys").stdin.read() if args.input == "-" else open(args.input, encoding="utf-8").read()
    try:
        result = validate(json.loads(raw))
    except (OSError, json.JSONDecodeError) as exc:
        result = {"schema": SCHEMA, "status": "error", "errors": [str(exc)]}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("status") == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
