"""Validate and aggregate the bounded host-preference experiment.

The experiment is intentionally telemetry-light: records contain assignment
and outcome metadata only.  Raw prompts, responses, source content, paths,
identities, and provider payloads are rejected at the boundary.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import tempfile
from collections import defaultdict
from pathlib import Path
from typing import Any, Iterable, Mapping

RECORD_SCHEMA = "simplicio.code-host-bias/v1"
AGGREGATE_SCHEMA = "simplicio.code-host-bias-aggregate/v1"
ROLLBACK_SCHEMA = "simplicio.code-host-bias-rollback/v1"
ALLOWED_HOSTS = frozenset({"claude", "codex"})
ALLOWED_VARIANTS = frozenset({"control", "bias"})
ALLOWED_ADOPTION = frozenset({"adopted", "not_adopted", "unknown"})
ALLOWED_ROLLBACK = frozenset({"not_requested", "kept", "rolled_back"})
RECORD_FIELDS = frozenset(
    {
        "schema",
        "experiment_id",
        "record_id",
        "host",
        "host_version",
        "variant",
        "text_revision",
        "consent",
        "adoption",
        "rollback",
        "recorded_at",
    }
)


def _text(value: Any, field: str) -> str:
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"{field} must be a non-empty string")
    return value.strip()


def validate_record(record: Mapping[str, Any]) -> dict[str, Any]:
    """Validate and return a stable copy of one redacted experiment record."""

    if not isinstance(record, Mapping):
        raise ValueError("record must be an object")
    unknown = sorted(set(record) - RECORD_FIELDS)
    if unknown:
        raise ValueError(f"record contains prohibited fields: {','.join(unknown)}")
    if _text(record.get("schema"), "schema") != RECORD_SCHEMA:
        raise ValueError("schema must be simplicio.code-host-bias/v1")
    for field in (
        "experiment_id",
        "record_id",
        "host_version",
        "text_revision",
        "recorded_at",
    ):
        _text(record.get(field), field)
    host = _text(record.get("host"), "host")
    if host not in ALLOWED_HOSTS:
        raise ValueError(f"host must be one of: {','.join(sorted(ALLOWED_HOSTS))}")
    variant = _text(record.get("variant"), "variant")
    if variant not in ALLOWED_VARIANTS:
        raise ValueError("variant must be control or bias")
    adoption = _text(record.get("adoption"), "adoption")
    if adoption not in ALLOWED_ADOPTION:
        raise ValueError("adoption must be adopted, not_adopted, or unknown")
    rollback = _text(record.get("rollback"), "rollback")
    if rollback not in ALLOWED_ROLLBACK:
        raise ValueError("rollback value is invalid")
    if record.get("consent") is not True:
        raise ValueError("consent must be true")
    return {
        "schema": RECORD_SCHEMA,
        "experiment_id": _text(record["experiment_id"], "experiment_id"),
        "record_id": _text(record["record_id"], "record_id"),
        "host": host,
        "host_version": _text(record["host_version"], "host_version"),
        "variant": variant,
        "text_revision": _text(record["text_revision"], "text_revision"),
        "consent": True,
        "adoption": adoption,
        "rollback": rollback,
        "recorded_at": _text(record["recorded_at"], "recorded_at"),
    }


def aggregate(records: Iterable[Mapping[str, Any]]) -> dict[str, Any]:
    """Return a deterministic, redacted adoption aggregate."""

    valid = [validate_record(record) for record in records]
    valid.sort(key=lambda item: item["record_id"])
    groups: dict[tuple[str, str, str], dict[str, int]] = defaultdict(
        lambda: {
            "records": 0,
            "adopted": 0,
            "not_adopted": 0,
            "unknown": 0,
            "not_requested": 0,
            "kept": 0,
            "rolled_back": 0,
        }
    )
    for record in valid:
        key = (record["host"], record["variant"], record["text_revision"])
        group = groups[key]
        group["records"] += 1
        group[record["adoption"]] += 1
        group[record["rollback"]] += 1

    summary = []
    for (host, variant, text_revision), counts in sorted(groups.items()):
        measured = counts["adopted"] + counts["not_adopted"]
        summary.append(
            {
                "host": host,
                "variant": variant,
                "text_revision": text_revision,
                **counts,
                "adoption_rate": (
                    counts["adopted"] / measured if measured else None
                ),
            }
        )
    return {
        "schema": AGGREGATE_SCHEMA,
        "experiment_ids": sorted({item["experiment_id"] for item in valid}),
        "record_count": len(valid),
        "redacted": True,
        "groups": summary,
    }


def _load_records(path: Path) -> list[dict[str, Any]]:
    raw = path.read_text(encoding="utf-8")
    if not raw.strip():
        return []
    try:
        value = json.loads(raw)
    except json.JSONDecodeError:
        value = [json.loads(line) for line in raw.splitlines() if line.strip()]
    if isinstance(value, Mapping):
        value = value.get("records")
    if not isinstance(value, list):
        raise ValueError("input must be a JSON array or an object with records")
    return [dict(item) for item in value]


def _digest(path: Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def rollback_text(active: Path, previous: Path, output: Path, reason: str) -> dict[str, Any]:
    """Atomically replace ``output`` with the previous text artifact."""

    if not reason.strip():
        raise ValueError("rollback reason must be non-empty")
    if not previous.is_file():
        raise ValueError("previous text artifact is missing")
    if active.exists() and not active.is_file():
        raise ValueError("active text artifact is not a file")
    output.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary_name = tempfile.mkstemp(prefix=f".{output.name}.", dir=output.parent)
    os.close(fd)
    temporary = Path(temporary_name)
    try:
        shutil.copyfile(previous, temporary)
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)
    return {
        "schema": ROLLBACK_SCHEMA,
        "decision": "rolled_back",
        "reason": reason.strip(),
        "active_sha256": _digest(active) if active.is_file() else None,
        "previous_sha256": _digest(previous),
        "result_sha256": _digest(output),
    }


def _dump(value: Mapping[str, Any]) -> None:
    print(json.dumps(value, ensure_ascii=False, sort_keys=True, separators=(",", ":")))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("validate", "aggregate"):
        command = commands.add_parser(name)
        command.add_argument("input", type=Path)
    rollback = commands.add_parser("rollback")
    rollback.add_argument("--active", required=True, type=Path)
    rollback.add_argument("--previous", required=True, type=Path)
    rollback.add_argument("--output", required=True, type=Path)
    rollback.add_argument("--reason", required=True)
    args = parser.parse_args(argv)
    try:
        if args.command == "validate":
            records = _load_records(args.input)
            for record in records:
                validate_record(record)
            _dump({"schema": RECORD_SCHEMA, "valid": True, "record_count": len(records)})
        elif args.command == "aggregate":
            _dump(aggregate(_load_records(args.input)))
        else:
            _dump(rollback_text(args.active, args.previous, args.output, args.reason))
        return 0
    except (OSError, ValueError) as error:
        _dump({"schema": "simplicio.code-host-bias-error/v1", "error": str(error)})
        return 2


if __name__ == "__main__":
    raise SystemExit(main())
