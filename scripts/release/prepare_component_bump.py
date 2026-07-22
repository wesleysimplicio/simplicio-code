#!/usr/bin/env python3
"""Fail-closed conversion of a signed ecosystem event into a Code bump.

The command deliberately does no discovery or downloading.  Its inputs are a
signed event, an operator-managed trust directory, and artifacts fetched by
the caller from its provenance endpoint.  A successful run atomically writes
the exact canonical manifest and an auditable receipt suitable for a bump PR.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from typing import Any

from scripts.validate_component_release import COMPONENTS, validate

EVENT_SCHEMA = "simplicio.release-event/v1"
STATE_SCHEMA = "simplicio.release-bump-state/v1"
TOKEN = re.compile(r"^[A-Za-z0-9._-]{1,256}$")


class BumpRejected(ValueError):
    """An event was rejected with an operator-actionable reason."""


def canonical(value: Any) -> bytes:
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()


def _verify_signature(payload: dict[str, Any], signature: str, key: Path) -> None:
    try:
        signature_bytes = base64.b64decode(signature, validate=True)
    except (ValueError, TypeError) as exc:
        raise BumpRejected("signature is not valid base64; republish the signed event") from exc
    with tempfile.TemporaryDirectory() as directory:
        payload_path = Path(directory, "payload.json")
        signature_path = Path(directory, "signature.bin")
        payload_path.write_bytes(canonical(payload))
        signature_path.write_bytes(signature_bytes)
        result = subprocess.run(
            ["openssl", "pkeyutl", "-verify", "-pubin", "-inkey", str(key),
             "-rawin", "-in", str(payload_path), "-sigfile", str(signature_path)],
            capture_output=True, text=True, check=False,
        )
    if result.returncode:
        raise BumpRejected("signature verification failed; verify publisher key and payload")


def prepare(event: dict[str, Any], trust_dir: Path, artifacts_dir: Path,
            current_state: dict[str, Any] | None = None) -> tuple[dict[str, Any], dict[str, Any]]:
    if set(event) != {"key_id", "signature", "payload"} or not isinstance(event.get("payload"), dict):
        raise BumpRejected("event envelope has unknown or missing fields; publish release-event/v1")
    key_id, payload = event.get("key_id"), event["payload"]
    if not isinstance(key_id, str) or not TOKEN.fullmatch(key_id) or "/" in key_id or "\\" in key_id:
        raise BumpRejected("key_id is invalid; use a trusted single-token key id")
    key_path = trust_dir / f"{key_id}.pem"
    if not key_path.is_file():
        raise BumpRejected(f"key {key_id!r} is missing or revoked; install an approved publisher key")
    if set(payload) != {"schema", "event_id", "producer", "sequence", "manifest", "bundle_digest"}:
        raise BumpRejected("event payload has unknown or missing fields; publish release-event/v1")
    if payload["schema"] != EVENT_SCHEMA:
        raise BumpRejected(f"event schema must be {EVENT_SCHEMA}")
    for field in ("event_id", "producer"):
        if not isinstance(payload[field], str) or not TOKEN.fullmatch(payload[field]):
            raise BumpRejected(f"{field} must be a non-empty single token")
    if isinstance(payload["sequence"], bool) or not isinstance(payload["sequence"], int) or payload["sequence"] < 1:
        raise BumpRejected("sequence must be a positive integer")
    _verify_signature(payload, event["signature"], key_path)

    result = validate(payload["manifest"])
    if result["status"] != "ready":
        raise BumpRejected("manifest is invalid or incompatible: " + "; ".join(result["errors"]))
    ranges = payload["manifest"]["compatibility"].get("protocol_ranges", {})
    for component in payload["manifest"]["components"]:
        match = re.fullmatch(r"(.+)/v([0-9]+)", component["protocol"])
        if match and match.group(1) in ranges:
            supported = ranges[match.group(1)]
            if not supported["min"] <= int(match.group(2)) <= supported["max"]:
                raise BumpRejected(
                    f"{component['name']} protocol is incompatible; publish an N/N-1 migration event"
                )
    if payload["bundle_digest"] != result["manifest_digest"]:
        raise BumpRejected("bundle digest is wrong; regenerate and re-sign the canonical manifest")
    for component in payload["manifest"]["components"]:
        artifact = artifacts_dir / component["name"]
        if not artifact.is_file():
            raise BumpRejected(f"artifact {component['name']} is missing; fetch its immutable publication")
        digest = hashlib.sha256(artifact.read_bytes()).hexdigest()
        if digest != component["artifact_digest"]:
            raise BumpRejected(f"artifact {component['name']} has the wrong digest; revoke or republish it")

    state = current_state or {"schema": STATE_SCHEMA, "events": []}
    if state.get("schema") != STATE_SCHEMA or not isinstance(state.get("events"), list):
        raise BumpRejected("bump history is invalid; restore it before processing events")
    for prior in state["events"]:
        if prior.get("event_id") == payload["event_id"]:
            if prior.get("bundle_digest") == payload["bundle_digest"]:
                return payload["manifest"], state
            raise BumpRejected("event id conflicts with bump history; rotate the event id")
        if prior.get("producer") == payload["producer"] and prior.get("sequence", 0) >= payload["sequence"]:
            raise BumpRejected("event sequence is stale; publish the next producer sequence")
    state = {"schema": STATE_SCHEMA, "events": [*state["events"], {
        "event_id": payload["event_id"], "producer": payload["producer"],
        "sequence": payload["sequence"], "bundle_digest": payload["bundle_digest"],
    }]}
    return payload["manifest"], state


def _atomic_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.{os.getpid()}.tmp")
    temporary.write_bytes(canonical(value) + b"\n")
    os.replace(temporary, path)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", type=Path, required=True)
    parser.add_argument("--trust-dir", type=Path, required=True)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument("--manifest-out", type=Path, required=True)
    parser.add_argument("--state", type=Path, required=True)
    args = parser.parse_args()
    try:
        event = json.loads(args.event.read_text(encoding="utf-8"))
        state = json.loads(args.state.read_text(encoding="utf-8")) if args.state.exists() else None
        manifest, next_state = prepare(event, args.trust_dir, args.artifacts_dir, state)
        _atomic_json(args.manifest_out, manifest)
        _atomic_json(args.state, next_state)
    except (OSError, json.JSONDecodeError, BumpRejected) as exc:
        print(json.dumps({"status": "blocked", "next_action": str(exc)}, sort_keys=True))
        return 2
    print(json.dumps({"status": "ready", "manifest_digest": event["payload"]["bundle_digest"]}, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
