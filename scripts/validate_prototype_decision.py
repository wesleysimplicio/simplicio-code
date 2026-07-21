#!/usr/bin/env python3
"""Validate the Prototype-First receipt and the fail-closed Build gate.

The Rust workspace-types crate is the runtime contract. This small Python
validator intentionally mirrors its security and decision rules so repository
preflight/headless tooling can validate a receipt without importing Rust.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from typing import Any

SCHEMA = "simplicio.prototype-decision/v1"
PREVIEW_SCHEMA = "simplicio.prototype-preview/v1"
DECISIONS = {"accept", "revise", "reject"}
ARTIFACT_TYPES = {
    "wireframe",
    "diagram",
    "schema",
    "data-model",
    "test-diff",
    "benchmark",
    "storyboard",
}
SAFE_ID = re.compile(r"^[A-Za-z0-9._-]{1,256}$")


def _safe_text(value: Any) -> bool:
    return isinstance(value, str) and not any(
        ord(char) < 32 and char not in "\n\t\r" for char in value
    )


def _safe_id(value: Any) -> bool:
    return isinstance(value, str) and bool(SAFE_ID.fullmatch(value))


def _safe_uri(value: Any) -> bool:
    if not isinstance(value, str) or not _safe_text(value) or not value:
        return False
    if value.startswith(("file:", "/", "\\")) or ".." in value.split("/"):
        return False
    return value.startswith(("artifact://", "runtime://")) or "://" not in value


def _decision_name(value: Any) -> str | None:
    if isinstance(value, str):
        return value if value in DECISIONS else None
    if isinstance(value, dict):
        kind = value.get("type")
        return kind if kind in DECISIONS else None
    return None


def _decision_note(value: Any) -> str:
    if isinstance(value, dict):
        payload = value.get("data") if isinstance(value.get("data"), dict) else value
        return str(payload.get("feedback" if value.get("type") == "revise" else "reason", ""))
    return ""


def validate(
    receipt: dict[str, Any],
    *,
    build_requested: bool = False,
    current_source_revision: str | None = None,
) -> dict[str, Any]:
    errors: list[str] = []
    if receipt.get("schema") != SCHEMA:
        errors.append(f"schema must be {SCHEMA}")
    for field in ("plan_id", "source_revision", "validated_source_revision", "decision_id"):
        if not _safe_text(receipt.get(field)) or not receipt[field].strip():
            errors.append(f"{field} is required and must be safe text")
    if receipt.get("source_revision") != receipt.get("validated_source_revision"):
        errors.append("source drift invalidates the prototype decision")
    if current_source_revision is not None and receipt.get("source_revision") != current_source_revision:
        errors.append("source drift invalidates the prototype decision")

    artifacts = receipt.get("artifacts")
    if not isinstance(artifacts, list) or not artifacts:
        errors.append("at least one prototype artifact is required")
        artifacts = []
    if len(artifacts) > 128:
        errors.append("too many artifacts (maximum 128)")
    artifact_ids: set[str] = set()
    for index, artifact in enumerate(artifacts):
        if not isinstance(artifact, dict):
            errors.append(f"artifacts[{index}] must be an object")
            continue
        artifact_id = artifact.get("id")
        if not _safe_id(artifact_id):
            errors.append(f"artifacts[{index}] id is required and must be safe")
        elif artifact_id in artifact_ids:
            errors.append(f"duplicate artifact id {artifact_id}")
        artifact_ids.add(str(artifact_id))
        if artifact.get("type") not in ARTIFACT_TYPES:
            errors.append(f"artifacts[{index}] has unsupported type")
        for field in ("title", "summary"):
            if not _safe_text(artifact.get(field)) or not artifact[field].strip():
                errors.append(f"artifacts[{index}] {field} is empty or unsafe")
        if not _safe_uri(artifact.get("uri")):
            errors.append(f"artifacts[{index}] uri escapes the artifact sandbox")
        if artifact.get("source_revision") != receipt.get("source_revision"):
            errors.append(f"artifacts[{index}] source revision differs from receipt")
        if not _safe_text(artifact.get("digest")) or not artifact["digest"].strip():
            errors.append(f"artifacts[{index}] digest is required")
        evidence = artifact.get("evidence")
        if not isinstance(evidence, list) or not evidence:
            errors.append(f"artifacts[{index}] requires evidence")
        else:
            for item in evidence:
                if not isinstance(item, dict) or not _safe_id(item.get("id")) or not _safe_text(item.get("label")) or not _safe_uri(item.get("uri")):
                    errors.append(f"artifact {artifact_id} contains unsafe evidence")
        coverage = artifact.get("ac_coverage")
        if not isinstance(coverage, list) or not coverage:
            errors.append(f"artifacts[{index}] requires AC coverage")

    for field in ("assumptions", "limitations", "provenance", "ac_coverage"):
        if not isinstance(receipt.get(field), list):
            errors.append(f"{field} must be a list")
    if isinstance(receipt.get("assumptions"), list) and any(not _safe_text(v) for v in receipt["assumptions"]):
        errors.append("assumptions contains unsafe text")
    if isinstance(receipt.get("limitations"), list) and any(not _safe_text(v) for v in receipt["limitations"]):
        errors.append("limitations contains unsafe text")
    if isinstance(receipt.get("provenance"), list) and any(not _safe_uri(v) for v in receipt["provenance"]):
        errors.append("provenance contains unsafe references")
    if not isinstance(receipt.get("ac_coverage"), list) or not receipt["ac_coverage"]:
        errors.append("acceptance-criteria coverage is required")

    decision = _decision_name(receipt.get("decision"))
    if decision is None:
        errors.append("decision must be accept, revise, or reject")
    elif decision in {"revise", "reject"} and not _decision_note(receipt.get("decision")).strip():
        errors.append(f"{decision} requires a note")
    stale = any("source drift" in error for error in errors)
    if stale:
        state = "stale"
    elif decision == "accept" and not errors:
        state = "build_authorized"
    elif decision == "revise":
        state = "revise_requested"
    elif decision == "reject":
        state = "rejected"
    elif not errors:
        state = "decision_pending"
    else:
        state = "blocked"
    authorized = build_requested and not errors and decision == "accept"
    if build_requested and not authorized:
        errors.append("Build requires a valid, current ACCEPT decision")
    canonical = json.dumps(receipt, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return {
        "schema": SCHEMA,
        "status": "ready" if not errors else "blocked",
        "state": state,
        "build_authorized": authorized,
        "errors": errors,
        "receipt_digest": hashlib.sha256(canonical).hexdigest(),
    }


def render(receipt: dict[str, Any], *, surface: str, current_source_revision: str | None = None) -> str:
    result = validate(receipt, current_source_revision=current_source_revision)
    if surface == "tui":
        decision = _decision_name(receipt.get("decision")) or "invalid"
        lines = [
            "PROTOTYPE PREVIEW",
            f"Plan: {receipt.get('plan_id', '')} | State: {result['state']} | Build: {'AUTHORIZED' if result['build_authorized'] else 'BLOCKED'}",
            f"Decision: {decision}",
            "Candidates:",
        ]
        artifacts = receipt.get("artifacts", [])
        if not isinstance(artifacts, list):
            artifacts = []
        for artifact in artifacts[:120]:
            if isinstance(artifact, dict):
                lines.append(f"  [{artifact.get('id', '')}] {artifact.get('type', '')}: {artifact.get('title', '')} — {artifact.get('summary', '')}")
        lines.append("Actions: [compare] [accept] [revise] [reject] [page]")
        if result["errors"]:
            lines.append("Blocked: " + "; ".join(result["errors"]))
        return "\n".join(lines)
    return json.dumps(
        {
            "schema": PREVIEW_SCHEMA,
            "surface": surface,
            "state": result["state"],
            "status": result["status"],
            "receipt": receipt,
            "actions": ["compare", "accept", "revise", "reject"],
            "build_authorized": result["build_authorized"],
            "errors": result["errors"],
        },
        indent=2,
        sort_keys=True,
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("receipt")
    parser.add_argument("--build", action="store_true")
    parser.add_argument("--source-revision")
    parser.add_argument("--surface", choices=("tui", "ui", "headless", "acp"))
    args = parser.parse_args(argv)
    try:
        receipt = json.loads(open(args.receipt, encoding="utf-8").read())
        result = validate(receipt, build_requested=args.build, current_source_revision=args.source_revision)
        if args.surface:
            print(render(receipt, surface=args.surface, current_source_revision=args.source_revision))
        else:
            print(json.dumps(result, indent=2, sort_keys=True))
    except (OSError, json.JSONDecodeError, TypeError) as exc:
        result = {"schema": SCHEMA, "status": "error", "errors": [str(exc)]}
        print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("status") == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
