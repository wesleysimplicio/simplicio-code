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


def validate(manifest: dict[str, Any]) -> dict[str, Any]:
    errors: list[str] = []
    if manifest.get("schema") != SCHEMA:
        errors.append("schema must be simplicio.component-release/v1")
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
        if name not in COMPONENTS:
            errors.append(f"components[{index}] has unknown name {name!r}")
        if name in names:
            errors.append(f"duplicate component {name}")
        names.add(name)
        version = item.get("version")
        if not isinstance(version, str) or not version or version in {"latest", "main", "dev"}:
            errors.append(f"{name} must have a pinned version")
        if not isinstance(item.get("commit"), str) or not re.fullmatch(r"[0-9a-f]{7,40}", item["commit"]):
            errors.append(f"{name} must have a pinned commit")
        if not isinstance(item.get("artifact_digest"), str) or not HEX64.fullmatch(item["artifact_digest"]):
            errors.append(f"{name} must have a sha256 artifact_digest")
        protocol = item.get("protocol")
        if not isinstance(protocol, str) or not protocol:
            errors.append(f"{name} must declare a protocol")
        else:
            protocols[str(name)] = protocol
    missing = COMPONENTS - names
    if missing:
        errors.append(f"missing components: {', '.join(sorted(missing))}")
    compatibility = manifest.get("compatibility")
    if not isinstance(compatibility, dict) or not compatibility.get("code_protocol"):
        errors.append("compatibility.code_protocol is required")
    canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":")).encode()
    return {
        "schema": SCHEMA,
        "status": "blocked" if errors else "ready",
        "errors": errors,
        "protocols": protocols,
        "manifest_digest": hashlib.sha256(canonical).hexdigest(),
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("manifest")
    args = parser.parse_args(argv)
    try:
        result = validate(json.loads(open(args.manifest, encoding="utf-8").read()))
    except (OSError, json.JSONDecodeError) as exc:
        result = {"schema": SCHEMA, "status": "error", "errors": [str(exc)]}
    print(json.dumps(result, indent=2, sort_keys=True))
    return 0 if result.get("status") == "ready" else 1


if __name__ == "__main__":
    raise SystemExit(main())
