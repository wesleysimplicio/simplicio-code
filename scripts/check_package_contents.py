#!/usr/bin/env python3
"""Check built/package trees for reintroduced internal JSON artifacts.

This is intentionally independent of Cargo/npm tooling: callers point it at a
staged install or package directory and it emits a compact Markdown report.
Only exact allowlisted paths from the reviewed TOML inventory are accepted;
directory-wide exceptions are not supported.
"""
from __future__ import annotations

import argparse
import datetime as dt
import pathlib
import sys
import tomllib

FORBIDDEN_SUFFIXES = {".json", ".jsonl", ".ndjson"}
PACKAGE_EXCEPTION_FIELDS = (
    "owner",
    "reason",
    "expires",
    "category",
    "target_format",
    "status",
    "producer",
    "consumer",
    "lifecycle",
)


def load_exact_inventory(path: pathlib.Path) -> set[str]:
    """Return exact, unexpired package-output exceptions from the inventory."""
    raw = tomllib.loads(path.read_text(encoding="utf-8"))
    paths: set[str] = set()
    for group in raw.get("package_output", []):
        names = group.get("paths") or ([group["path"]] if group.get("path") else [])
        for name in names:
            candidate = pathlib.PurePosixPath(name)
            if (
                not name
                or candidate.is_absolute()
                or any(part in {"", ".", ".."} for part in candidate.parts)
                or candidate.as_posix() != name
                or any(ch in name for ch in "*?[]")
            ):
                raise ValueError(f"package inventory path must be exact: {name!r}")
            for field in PACKAGE_EXCEPTION_FIELDS:
                if not group.get(field):
                    raise ValueError(f"{name}: missing {field}")
            if group["status"] != "exception":
                raise ValueError(f"{name}: package output must be an exception")
            try:
                expires = dt.date.fromisoformat(group["expires"])
            except ValueError as exc:
                raise ValueError(f"{name}: invalid expires date") from exc
            if expires < dt.date.today():
                raise ValueError(f"{name}: package exception expired on {group['expires']}")
            if name in paths:
                raise ValueError(f"duplicate package inventory path: {name}")
            paths.add(name)
    return paths


def violations(root: pathlib.Path, inventory: set[str]) -> list[str]:
    result: list[str] = []
    for path in sorted(p for p in root.rglob("*") if p.is_file()):
        rel = path.relative_to(root).as_posix()
        if path.suffix.lower() not in FORBIDDEN_SUFFIXES:
            continue
        if rel not in inventory:
            result.append(rel)
    return result


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("root", type=pathlib.Path)
    parser.add_argument("--inventory", type=pathlib.Path, required=True)
    args = parser.parse_args()
    try:
        inventory = load_exact_inventory(args.inventory)
        found = violations(args.root, inventory)
    except (OSError, ValueError, tomllib.TOMLDecodeError) as exc:
        print(f"package scan error: {exc}", file=sys.stderr)
        return 2
    print("# Package JSON boundary scan\n")
    print(f"- Root: `{args.root}`  ")
    print(f"- JSON artifacts not exact-package-allowlisted: `{len(found)}`\n")
    if found:
        print("| Path |\n|---|")
        for path in found:
            print(f"| `{path}` |")
    return 1 if found else 0


if __name__ == "__main__":
    raise SystemExit(main())
