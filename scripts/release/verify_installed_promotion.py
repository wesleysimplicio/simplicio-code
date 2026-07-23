#!/usr/bin/env python3
"""Verify that an installed bundle came from one signed release event.

This verifier is deliberately offline: it neither discovers releases nor treats
test fixtures as ecosystem publications.  It replays the existing signature,
manifest, and artifact checks and then binds those inputs to the active slot and
the receipts produced by the bump and promotion steps.
"""

from __future__ import annotations

import argparse
import hashlib
import json
from pathlib import Path
from typing import Any

from scripts.release.prepare_component_bump import BumpRejected, canonical, prepare


class EvidenceRejected(ValueError):
    """Installed evidence cannot be reconstructed from the signed inputs."""


def _read_json(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise EvidenceRejected(f"{path} must contain a JSON object")
    return value


def verify(event: dict[str, Any], trust_dir: Path, artifacts_dir: Path,
           schema: Path, active_slot: Path, bump_receipt: dict[str, Any],
           promotion_receipt: dict[str, Any]) -> dict[str, Any]:
    """Return deterministic evidence, or reject any broken provenance link."""
    manifest, _, replayed_receipt = prepare(event, trust_dir, artifacts_dir, schema)
    if not isinstance(bump_receipt.get("duplicate", False), bool):
        raise EvidenceRejected("bump receipt duplicate marker must be boolean")
    expected_bump = {**replayed_receipt, "duplicate": bump_receipt.get("duplicate", False)}
    if bump_receipt != expected_bump:
        raise EvidenceRejected("bump receipt does not match the verified release event")

    installed_manifest_path = active_slot / "manifest.json"
    if not installed_manifest_path.is_file():
        raise EvidenceRejected("active slot has no manifest.json")
    installed_manifest = _read_json(installed_manifest_path)
    if canonical(installed_manifest) != canonical(manifest):
        raise EvidenceRejected("installed manifest does not match the signed release event")

    bundle_digest = event["payload"]["bundle_digest"]
    required_promotion = {
        "schema": "simplicio.bundle-promotion-receipt/v1",
        "decision": "promoted",
        "active_digest": bundle_digest,
    }
    if any(promotion_receipt.get(key) != value for key, value in required_promotion.items()):
        raise EvidenceRejected("promotion receipt does not identify the signed active bundle")

    installed_digests: dict[str, str] = {}
    for component in manifest["components"]:
        installed = active_slot / component["name"]
        if not installed.is_file():
            raise EvidenceRejected(f"installed component {component['name']} is missing")
        digest = hashlib.sha256(installed.read_bytes()).hexdigest()
        if digest != component["artifact_digest"]:
            raise EvidenceRejected(f"installed component {component['name']} has the wrong digest")
        installed_digests[component["name"]] = digest

    # No timestamps, host names, or network observations: identical evidence
    # replays byte-for-byte on another machine with the same installed inputs.
    return {
        "schema": "simplicio.installed-release-evidence/v1",
        "decision": "verified-installed",
        "event_id": event["payload"]["event_id"],
        "producer": event["payload"]["producer"],
        "sequence": event["payload"]["sequence"],
        "signing_key_id": event["key_id"],
        "signed_payload_digest": hashlib.sha256(canonical(event["payload"])).hexdigest(),
        "bundle_digest": bundle_digest,
        "installed_artifact_digests": installed_digests,
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--event", type=Path, required=True)
    parser.add_argument("--trust-dir", type=Path, required=True)
    parser.add_argument("--artifacts-dir", type=Path, required=True)
    parser.add_argument("--schema", type=Path,
                        default=Path("docs/contracts/component-release-v1.schema.json"))
    parser.add_argument("--active-slot", type=Path, required=True)
    parser.add_argument("--bump-receipt", type=Path, required=True)
    parser.add_argument("--promotion-receipt", type=Path, required=True)
    parser.add_argument("--out", type=Path)
    args = parser.parse_args()
    try:
        evidence = verify(
            _read_json(args.event), args.trust_dir, args.artifacts_dir, args.schema,
            args.active_slot, _read_json(args.bump_receipt), _read_json(args.promotion_receipt),
        )
        encoded = canonical(evidence) + b"\n"
        if args.out:
            args.out.parent.mkdir(parents=True, exist_ok=True)
            args.out.write_bytes(encoded)
        print(encoded.decode(), end="")
        return 0
    except (OSError, json.JSONDecodeError, BumpRejected, EvidenceRejected) as exc:
        print(json.dumps({"status": "blocked", "next_action": str(exc)}, sort_keys=True))
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
