#!/usr/bin/env python3
"""Validate Prototype-First artifacts and the human gate before Build."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from typing import Any

SCHEMA = "simplicio.prototype-decision/v1"
DECISIONS = {"accept", "revise", "reject"}
ARTIFACT_TYPES = {"wireframe", "diagram", "schema", "data-model", "test-diff", "benchmark", "storyboard"}


def _safe_text(value: Any) -> bool:
    if not isinstance(value, str):
        return True
    return not any(ord(char) < 32 and char not in "\n\t\r" for char in value)


def validate(receipt: dict[str, Any], *, build_requested: bool = False) -> dict[str, Any]:
    errors: list[str] = []
    if receipt.get("schema") != SCHEMA:
        errors.append("schema must be simplicio.prototype-decision/v1")
    for field in ("plan_id", "source_revision", "decision_id"):
        if not isinstance(receipt.get(field), str) or not receipt[field].strip():
            errors.append(f"{field} is required")
    artifacts = receipt.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        errors.append("at least one prototype artifact is required")
        artifacts = []
    artifact_ids: set[str] = set()
    for index, artifact in enumerate(artifacts):
        if not isinstance(artifact, dict):
            errors.append(f"artifacts[{index}] must be an object")
            continue
        artifact_id = artifact.get("id")
        if not isinstance(artifact_id, str) or not artifact_id:
            errors.append(f"artifacts[{index}] id is required")
        elif artifact_id in artifact_ids:
            errors.append(f"duplicate artifact id {artifact_id}")
        artifact_ids.add(str(artifact_id))
        if artifact.get("type") not in ARTIFACT_TYPES:
            errors.append(f"artifacts[{index}] has unsupported type")
        if not _safe_text(artifact.get("title")) or not _safe_text(artifact.get("summary")):
            errors.append(f"artifacts[{index}] contains unsafe control characters")
        if isinstance(artifact.get("uri"), str) and (".." in artifact["uri"].split("/") or artifact["uri"].startswith("file://")):
            errors.append(f"artifacts[{index}] uri escapes the artifact sandbox")
    decision = receipt.get("decision")
    if decision not in DECISIONS:
        errors.append("decision must be accept, revise, or reject")
    if not isinstance(receipt.get("assumptions"), list) or not isinstance(receipt.get("limitations"), list):
        errors.append("assumptions and limitations must be lists")
    if receipt.get("source_revision") != receipt.get("validated_source_revision"):
        errors.append("source drift invalidates the prototype decision")
    if build_requested and (errors or decision != "accept"):
        errors.append("Build requires a valid, current ACCEPT decision")
    canonical = json.dumps(receipt, sort_keys=True, separators=(",", ":")).encode()
    return {"schema": SCHEMA, "status": "blocked" if errors else "ready", "build_authorized": build_requested and not errors, "errors": errors, "receipt_digest": hashlib.sha256(canonical).hexdigest()}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("receipt")
    parser.add_argument("--build", action="store_true")
    args = parser.parse_args(argv)
    try:
        result = validate(json.loads(open(args.receipt, encoding="utf-8").read()), build_requested=args.build)
    except (OSError, json.JSONDecodeError) as exc:
        result = {"schema": SCHEMA, "status": "error", "errors": [str(exc)]}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("status") == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
