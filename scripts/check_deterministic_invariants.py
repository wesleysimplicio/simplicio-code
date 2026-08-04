#!/usr/bin/env python3
"""Enforce small, deterministic Code-owned initializer invariants (#324)."""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path


INVARIANTS = (
    {
        "path": Path("crates/codegen/xai-grok-shell/src/session/acp_session_impl/spawn.rs"),
        "initializer": "AgentRebuildSpec",
        "fields": ("fs_backend", "search_backend", "directory_backend"),
    },
)


def _initializer_body(source: str, initializer: str) -> str | None:
    match = re.search(rf"\b{re.escape(initializer)}\s*\{{", source)
    if not match:
        return None
    start = match.end() - 1
    depth = 0
    for index in range(start, len(source)):
        char = source[index]
        if char == "{":
            depth += 1
        elif char == "}":
            depth -= 1
            if depth == 0:
                return source[start + 1 : index]
    return None


def check(root: Path) -> list[dict[str, object]]:
    findings: list[dict[str, object]] = []
    for invariant in INVARIANTS:
        path = root / invariant["path"]
        if not path.is_file():
            findings.append({"path": str(invariant["path"]), "reason": "source file missing"})
            continue
        body = _initializer_body(path.read_text(encoding="utf-8"), str(invariant["initializer"]))
        if body is None:
            findings.append({"path": str(invariant["path"]), "reason": "initializer missing"})
            continue
        for field in invariant["fields"]:
            if not re.search(rf"(?m)^\s*{re.escape(field)}\s*(?::|,)\s*", body):
                findings.append(
                    {
                        "path": str(invariant["path"]),
                        "initializer": invariant["initializer"],
                        "field": field,
                        "reason": "required field is absent",
                    }
                )
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path("."))
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    findings = check(args.root.resolve())
    report = {
        "schema": "simplicio.deterministic-invariants/v1",
        "status": "FAIL" if findings else "PASS",
        "invariant_count": len(INVARIANTS),
        "findings": findings,
    }
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else json.dumps(report))
    return 1 if findings else 0


if __name__ == "__main__":
    raise SystemExit(main())
