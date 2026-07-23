#!/usr/bin/env python3
"""Validate the pinned, provenance-bearing Code release bundle manifest."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from typing import Any

SCHEMA = "simplicio.component-release/v1"
COMPONENTS = {"code", "runtime", "loop-hub", "agent-contracts"}
HEX64 = re.compile(r"^[0-9a-f]{64}$")
COMMIT = re.compile(r"^[0-9a-f]{40}$")
HANDSHAKE_SCHEMA = "simplicio.compatibility-handshake/v1"


def validate(manifest: dict[str, Any]) -> dict[str, Any]:
    errors: list[str] = []
    if not isinstance(manifest, dict):
        return {
            "schema": SCHEMA,
            "handshake_schema": HANDSHAKE_SCHEMA,
            "status": "blocked",
            "errors": ["manifest must be an object"],
            "protocols": {},
            "component_digests": {},
            "next_action": "publish a pinned compatible bundle",
            "manifest_digest": hashlib.sha256(
                json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
            ).hexdigest(),
        }
    if manifest.get("schema") != SCHEMA:
        errors.append("schema must be simplicio.component-release/v1")
    bundle_version = manifest.get("bundle_version")
    if not isinstance(bundle_version, str) or not bundle_version or bundle_version in {"latest", "main", "dev"}:
        errors.append("bundle_version must be pinned")
    components = manifest.get("components")
    if not isinstance(components, list):
        errors.append("components must be a list")
        components = []
    names: set[str] = set()
    protocols: dict[str, str] = {}
    for index, item in enumerate(components):
        if not isinstance(item, dict):
            errors.append(f"components[{index}] must be an object")
            continue
        name = item.get("name")
        if not isinstance(name, str) or name not in COMPONENTS:
            errors.append(f"components[{index}] has unknown name {name!r}")
        if isinstance(name, str) and name in names:
            errors.append(f"duplicate component {name}")
        if isinstance(name, str):
            names.add(name)
        version = item.get("version")
        if not isinstance(version, str) or not version or version in {"latest", "main", "dev"}:
            errors.append(f"{name} must have a pinned version")
        if not isinstance(item.get("commit"), str) or not COMMIT.fullmatch(item["commit"]):
            errors.append(f"{name} must have a pinned commit")
        if not isinstance(item.get("artifact_digest"), str) or not HEX64.fullmatch(item["artifact_digest"]):
            errors.append(f"{name} must have a sha256 artifact_digest")
        protocol = item.get("protocol")
        if not isinstance(protocol, str) or not protocol:
            errors.append(f"{name} must declare a protocol")
        else:
            protocols[str(name)] = protocol
        generated_digest = item.get("generated_client_digest")
        if name == "runtime" and generated_digest is None:
            errors.append("runtime must have a sha256 generated_client_digest")
        elif generated_digest is not None and (
            not isinstance(generated_digest, str) or not HEX64.fullmatch(generated_digest)
        ):
            errors.append(f"{name} must have a sha256 generated_client_digest")
    missing = COMPONENTS - names
    if missing:
        errors.append(f"missing components: {', '.join(sorted(missing))}")
    compatibility = manifest.get("compatibility")
    if not isinstance(compatibility, dict) or not compatibility.get("code_protocol"):
        errors.append("compatibility.code_protocol is required")
    protocol_ranges = compatibility.get("protocol_ranges", {}) if isinstance(compatibility, dict) else {}
    if not isinstance(protocol_ranges, dict):
        errors.append("compatibility.protocol_ranges must be an object")
    else:
        for family, value in protocol_ranges.items():
            if (
                not isinstance(family, str)
                or not isinstance(value, dict)
                or not isinstance(value.get("min"), int)
                or not isinstance(value.get("max"), int)
                or value["min"] < 0
                or value["min"] > value["max"]
            ):
                errors.append(f"invalid compatibility.protocol_ranges entry: {family!r}")
    canonical = json.dumps(
        manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False
    ).encode()
    component_digests = {
        item["name"]: item.get("artifact_digest")
        for item in components
        if isinstance(item, dict)
        and isinstance(item.get("name"), str)
        and item["name"] in COMPONENTS
    }
    return {
        "schema": SCHEMA,
        "handshake_schema": HANDSHAKE_SCHEMA,
        "status": "blocked" if errors else "ready",
        "errors": errors,
        "protocols": protocols,
        "component_digests": component_digests,
        "next_action": "publish a pinned compatible bundle" if errors else "safe to stage and canary",
        "manifest_digest": hashlib.sha256(canonical).hexdigest(),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest")
    args = parser.parse_args(argv)
    try:
        with open(args.manifest, encoding="utf-8") as manifest_file:
            result = validate(json.load(manifest_file))
    except (OSError, json.JSONDecodeError) as exc:
        result = {"schema": SCHEMA, "status": "error", "errors": [str(exc)]}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("status") == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
