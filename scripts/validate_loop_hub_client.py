#!/usr/bin/env python3
"""Fail-closed admission contract for Code as an interactive Loop Hub client."""

from __future__ import annotations

import argparse
import json
from typing import Any

SCHEMA = "simplicio.loop-hub-client/v1"
MODES = {"auto", "hub", "required", "standalone"}
OWNERS = {"loop-hub", "code-process"}
SHARED_SERVICES = {"runtime", "mapper", "scheduler", "inference"}


def validate(status: dict[str, Any]) -> dict[str, Any]:
    errors: list[str] = []
    if status.get("schema", SCHEMA) != SCHEMA:
        errors.append(f"unsupported schema {status.get('schema')!r}")
    mode = status.get("mode", "auto")
    hub = status.get("hub") or {}
    agent = status.get("agent") or {}
    runtime = status.get("runtime") or {}
    workflow = status.get("workflow") or {"scope": "single"}
    if mode not in MODES:
        errors.append("mode must be auto, hub, required, or standalone")
    if mode in {"hub", "required"} and hub.get("state") != "ready":
        errors.append("hub/required mode needs a ready Loop Hub")
    if mode == "standalone" and hub.get("state") == "ready":
        errors.append("standalone mode cannot attach to a ready Hub")
    if agent.get("state") != "ready" or not agent.get("protocol"):
        errors.append("production Code requires a ready, versioned Simplicio Agent")
    if runtime.get("state") != "ready" or not runtime.get("protocol"):
        errors.append("production Code requires a ready, versioned Simplicio Runtime")
    if workflow.get("scope") not in {"single", "coordinated"}:
        errors.append("workflow scope must be single or coordinated")
    if workflow.get("scope") == "coordinated" and hub.get("state") != "ready":
        errors.append("coordinated goals require Loop Hub; local scheduling is forbidden")
    services = status.get("services") or []
    seen: dict[str, list[str]] = {}
    for service in services:
        if not isinstance(service, dict) or service.get("name") not in SHARED_SERVICES:
            errors.append("services must name runtime, mapper, scheduler, or inference")
            continue
        name = service["name"]
        owner = service.get("owner")
        seen.setdefault(name, []).append(str(owner))
        if owner not in OWNERS:
            errors.append(f"{name} has invalid owner {owner!r}")
        if hub.get("state") == "ready" and owner == "code-process":
            errors.append(f"{name} must be reused from Loop Hub when Hub is ready")
    for name, owners in seen.items():
        if len(owners) != len(set(owners)):
            errors.append(f"{name} has duplicate service declarations")
        if hub.get("state") == "ready" and owners.count("loop-hub") != 1:
            errors.append(f"{name} needs exactly one Loop Hub owner")
    if hub.get("state") == "ready":
        for name in SHARED_SERVICES:
            if name not in seen:
                errors.append(f"{name} needs exactly one Loop Hub owner")
    return {
        "schema": SCHEMA,
        "status": "blocked" if errors else "ready",
        "effective_mode": "hub" if hub.get("state") == "ready" and mode == "auto" else mode,
        "errors": errors,
    }


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
