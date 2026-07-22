#!/usr/bin/env python3
"""Stage, canary, atomically promote, and roll back one verified bundle.

This module never discovers versions or downloads artifacts.  The caller must
first validate a signed release event; this boundary independently rechecks the
canonical manifest and every artifact digest before changing the active slot.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
from typing import Callable

from scripts.validate_component_release import validate


class PromotionRejected(RuntimeError):
    pass


def _digest(path: Path) -> str:
    with path.open("rb") as stream:
        digest = hashlib.sha256()
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def promote(manifest: dict, artifacts: Path, slots: Path,
            canary: Callable[[Path], bool]) -> dict:
    result = validate(manifest)
    if result["status"] != "ready":
        raise PromotionRejected("manifest rejected: " + "; ".join(result["errors"]))
    slots.mkdir(parents=True, exist_ok=True)
    lock = slots / ".promotion.lock"
    try:
        descriptor = os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    except FileExistsError as exc:
        raise PromotionRejected("another promotion is in progress") from exc
    os.close(descriptor)
    stage: Path | None = None
    try:
        stage = Path(tempfile.mkdtemp(prefix=".inactive-", dir=slots))
        for component in manifest["components"]:
            source = artifacts / component["name"]
            if not source.is_file():
                raise PromotionRejected(f"artifact {component['name']} is missing")
            if _digest(source) != component["artifact_digest"]:
                raise PromotionRejected(f"artifact {component['name']} digest mismatch")
            target = stage / component["name"]
            shutil.copyfile(source, target)
            target.chmod(0o500)
            if _digest(target) != component["artifact_digest"]:
                raise PromotionRejected(f"staged artifact {component['name']} digest mismatch")
        (stage / "manifest.json").write_text(
            json.dumps(manifest, sort_keys=True, separators=(",", ":")) + "\n",
            encoding="utf-8",
        )
        if not canary(stage):
            raise PromotionRejected("inactive-slot canary failed; active bundle is unchanged")
        for component in manifest["components"]:
            if _digest(stage / component["name"]) != component["artifact_digest"]:
                raise PromotionRejected(
                    f"canary modified {component['name']}; active bundle is unchanged"
                )

        active, previous = slots / "active", slots / "previous"
        if previous.exists():
            shutil.rmtree(previous)
        if active.exists():
            os.replace(active, previous)
        try:
            os.replace(stage, active)
        except BaseException:
            if previous.exists() and not active.exists():
                os.replace(previous, active)
            raise
        stage = None
        return {"schema": "simplicio.bundle-promotion-receipt/v1",
                "decision": "promoted", "active_digest": result["manifest_digest"],
                "rollback_available": previous.is_dir()}
    finally:
        if stage is not None:
            shutil.rmtree(stage, ignore_errors=True)
        lock.unlink(missing_ok=True)


def rollback(slots: Path) -> dict:
    active, previous = slots / "active", slots / "previous"
    if not active.is_dir() or not previous.is_dir():
        raise PromotionRejected("rollback requires both active and previous slots")
    temporary = slots / ".rollback"
    if temporary.exists():
        raise PromotionRejected("stale rollback slot requires operator recovery")
    os.replace(active, temporary)
    try:
        os.replace(previous, active)
        os.replace(temporary, previous)
    except BaseException:
        if temporary.exists() and not active.exists():
            os.replace(temporary, active)
        raise
    manifest = json.loads((active / "manifest.json").read_text(encoding="utf-8"))
    return {"schema": "simplicio.bundle-promotion-receipt/v1",
            "decision": "rolled_back", "active_digest": validate(manifest)["manifest_digest"]}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("operation", choices=("promote", "rollback"))
    parser.add_argument("--slots", type=Path, required=True)
    parser.add_argument("--manifest", type=Path)
    parser.add_argument("--artifacts", type=Path)
    parser.add_argument("--canary-command", nargs="+")
    args = parser.parse_args()
    try:
        if args.operation == "rollback":
            receipt = rollback(args.slots)
        else:
            if not args.manifest or not args.artifacts or not args.canary_command:
                raise PromotionRejected("promotion requires manifest, artifacts, and canary command")
            manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
            def canary(stage: Path) -> bool:
                environment = {**os.environ, "SIMPLICIO_BUNDLE_SLOT": str(stage)}
                return subprocess.run(args.canary_command, env=environment, check=False).returncode == 0
            receipt = promote(manifest, args.artifacts, args.slots, canary)
    except (OSError, json.JSONDecodeError, PromotionRejected) as exc:
        print(json.dumps({"status": "blocked", "next_action": str(exc)}, sort_keys=True))
        return 2
    print(json.dumps(receipt, sort_keys=True))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
