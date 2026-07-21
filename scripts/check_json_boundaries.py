#!/usr/bin/env python3
"""Audit JSON use against the exact boundary inventory.

The baseline mode is intentionally non-blocking while the Runtime publishes
HBI v1. Strict mode is the release gate: every finding must be an explicit
exception or already migrated to HBP/HBI/TOML. Paths in the inventory are
exact; glob entries are rejected.
"""
from __future__ import annotations

import argparse
import datetime as dt
import pathlib
import re
import sys
import tomllib

TOKENS = re.compile(
    r"(?:import\s+json|from\s+json\s+import|json\.(?:loads|dumps|load|dump)|"
    r"JSON\.(?:parse|stringify)|serde_json|\.jsonl?\b|\.ndjson\b|jsonrpc)",
    re.IGNORECASE,
)
SKIP = {".git", "node_modules", "target", "dist", "build", ".venv", "__pycache__", ".orchestrator"}
SOURCE_SUFFIXES = {".py", ".mjs", ".js", ".ts", ".tsx", ".rs", ".go", ".java", ".cs"}


def load_inventory(path: pathlib.Path) -> dict[str, dict]:
    raw = tomllib.loads(path.read_text(encoding="utf-8"))
    result: dict[str, dict] = {}
    # v1 inventories may group exact paths under [[boundary]]/[[audit]].
    # Expand those groups into the same normalized map used by the scanner.
    groups = list(raw.get("boundary", [])) + list(raw.get("audit", []))
    for group in groups:
        names = group.get("paths") or ([group["path"]] if group.get("path") else [])
        target = group.get("target_format", "")
        status = group.get("status") or (
            "exception" if target.lower().startswith(("preserve", "json-rpc", "external", "signed", "cyclonedx"))
            else "migration_pending"
        )
        entry = {
            **group,
            "status": status,
            "reason": group.get("reason") or group.get("rationale") or group.get("lifecycle", ""),
            "expires": group.get("expires") or ("2099-12-31" if status == "exception" else "2026-12-31"),
        }
        for name in names:
            if not name or any(ch in name for ch in "*?[]"):
                raise ValueError(f"inventory path must be exact: {name!r}")
            if name in result:
                raise ValueError(f"duplicate inventory path: {name}")
            for field in ("owner", "reason", "expires", "category", "target_format", "status"):
                if not entry.get(field):
                    raise ValueError(f"{name}: missing {field}")
            try:
                dt.date.fromisoformat(entry["expires"])
            except ValueError as exc:
                raise ValueError(f"{name}: invalid expires date") from exc
            result[name] = entry
    return result


def findings(root: pathlib.Path) -> list[tuple[str, int, str]]:
    out: list[tuple[str, int, str]] = []
    for path in root.rglob("*"):
        rel_path = path.relative_to(root)
        if (not path.is_file() or any(part in SKIP for part in rel_path.parts)
                or path.suffix.lower() not in SOURCE_SUFFIXES
                or rel_path.as_posix() == "scripts/check_json_boundaries.py"):
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except (UnicodeDecodeError, OSError):
            continue
        matches = [m for m in TOKENS.finditer(text)]
        if matches:
            rel = path.relative_to(root).as_posix()
            out.extend((rel, text.count("\n", 0, m.start()) + 1, m.group(0)) for m in matches)
    return out


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).parents[1])
    parser.add_argument("--inventory", type=pathlib.Path)
    parser.add_argument("--mode", choices=("baseline", "strict"), default="baseline")
    parser.add_argument("--max-findings", type=int, default=50)
    args = parser.parse_args()
    inventory_path = args.inventory or args.root / "config" / "json-boundaries.toml"
    try:
        inventory = load_inventory(inventory_path)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"inventory error: {exc}", file=sys.stderr)
        return 2
    unknown: list[tuple[str, int, str]] = []
    pending: list[tuple[str, int, str]] = []
    expired: list[str] = []
    today = dt.date.today()
    for path, line, token in findings(args.root):
        entry = inventory.get(path)
        if entry is None:
            unknown.append((path, line, token))
        elif dt.date.fromisoformat(entry["expires"]) < today:
            expired.append(path)
        elif entry["status"] == "migration_pending":
            pending.append((path, line, token))
    print(f"json-boundaries: findings={len(unknown)+len(pending)} unknown={len(unknown)} pending={len(pending)} expired={len(expired)}")
    for path, line, token in unknown[: max(0, args.max_findings)]:
        print(f"UNCLASSIFIED {path}:{line}: {token}")
    for path in expired:
        print(f"EXPIRED {path}")
    if args.mode == "strict" and (unknown or pending or expired):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
