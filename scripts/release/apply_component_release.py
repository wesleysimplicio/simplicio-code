#!/usr/bin/env python3
"""Apply one verified ecosystem release event to the Code release lock.

The GitHub workflow is the authentication boundary for ``repository_dispatch``;
this program is deliberately offline and additionally verifies every digest.
It never resolves a floating version or downloads an artifact.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import tempfile
from pathlib import Path
from typing import Any

from scripts.release.generate_component_client import render
from scripts.validate_component_release import validate

EVENT_SCHEMA = "simplicio.release-event/v1"
PRODUCERS = {"simplicio-agent", "simplicio-loop", "simplicio-runtime"}


def canonical_digest(value: Any) -> str:
    payload = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
    return hashlib.sha256(payload).hexdigest()


def prepare(event: dict[str, Any], schema: Path, current: Path | None = None) -> tuple[dict[str, Any], str, dict[str, Any]]:
    if event.get("schema") != EVENT_SCHEMA:
        raise ValueError(f"schema must be {EVENT_SCHEMA}")
    if event.get("producer") not in PRODUCERS:
        raise ValueError("release event producer is not trusted")
    if not isinstance(event.get("sequence"), int) or event["sequence"] < 1:
        raise ValueError("release event sequence must be positive")
    manifest = event.get("manifest")
    result = validate(manifest)
    if result["status"] != "ready":
        raise ValueError("invalid component release: " + "; ".join(result["errors"]))
    digest = canonical_digest(manifest)
    if event.get("bundle_digest") != digest:
        raise ValueError("release event bundle_digest does not match its canonical manifest")

    generated = render(schema)
    generated_digest = hashlib.sha256(generated.encode()).hexdigest()
    runtime = next(component for component in manifest["components"] if component["name"] == "runtime")
    if runtime.get("generated_client_digest") != generated_digest:
        raise ValueError("runtime generated_client_digest does not match reproducible bindings")

    previous_digest = None
    migration_required = False
    if current and current.is_file():
        previous = json.loads(current.read_text(encoding="utf-8"))
        installed_digest = canonical_digest(previous)
        previous_digest = installed_digest if installed_digest != digest else None
        migration_required = previous.get("compatibility") != manifest.get("compatibility")
    receipt = {
        "schema": "simplicio.release-lock-receipt/v1",
        "event_id": event.get("event_id"),
        "producer": event["producer"],
        "sequence": event["sequence"],
        "previous_digest": previous_digest,
        "active_digest": digest,
        "generated_client_digest": generated_digest,
        "migration_required": migration_required,
    }
    return manifest, generated, receipt


def atomic_write(path: Path, data: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    try:
        with os.fdopen(fd, "w", encoding="utf-8") as output:
            output.write(data)
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("event", type=Path)
    parser.add_argument("--schema", type=Path, default=Path("docs/contracts/component-release-v1.schema.json"))
    parser.add_argument("--manifest", type=Path, default=Path("release/component-release.json"))
    parser.add_argument("--generated", type=Path, default=Path("crates/codegen/simplicio-runtime-client/src/generated.rs"))
    parser.add_argument("--receipt", type=Path, default=Path("release/component-release.receipt.json"))
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args(argv)
    event = json.loads(args.event.read_text(encoding="utf-8"))
    manifest, generated, receipt = prepare(event, args.schema, args.manifest)
    outputs = {
        args.manifest: json.dumps(manifest, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
        args.generated: generated,
        args.receipt: json.dumps(receipt, indent=2, sort_keys=True, ensure_ascii=False) + "\n",
    }
    if args.check:
        stale = [str(path) for path, content in outputs.items() if not path.is_file() or path.read_text(encoding="utf-8") != content]
        if stale:
            raise SystemExit("stale release outputs: " + ", ".join(stale))
        return 0
    for path, content in outputs.items():
        atomic_write(path, content)
    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
