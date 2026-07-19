#!/usr/bin/env python3
"""Report likely Rust struct-initializer omissions (#68).

This is intentionally a review report, not a hard-fail parser: Rust syntax is
too rich for a regex-only detector to prove ownership of every local variable.
It catches the high-value shape behind code#64 and emits candidates for human
review without blocking unrelated upstream crates.
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path
from typing import Iterable

LET_RE = re.compile(r"\blet\s+(?!_)([A-Za-z][A-Za-z0-9_]*)\s*=")
STRUCT_RE = re.compile(r"\b[A-Z][A-Za-z0-9_]*\s*\{")


def scan_source(source: str, path: str = "<source>") -> list[dict[str, object]]:
    lines = source.splitlines()
    candidates: list[dict[str, object]] = []
    for index, line in enumerate(lines):
        match = LET_RE.search(line)
        if not match:
            continue
        name = match.group(1)
        # Limit the report to a nearby initializer. This keeps the detector
        # review-oriented and avoids claiming a distant use belongs to the
        # same function without pretending to be a Rust parser.
        window_end = min(len(lines), index + 81)
        for struct_index in range(index + 1, window_end):
            if not STRUCT_RE.search(lines[struct_index]):
                continue
            brace_balance = 0
            block: list[str] = []
            for body_index in range(struct_index, window_end):
                text = lines[body_index]
                block.append(text)
                brace_balance += text.count("{") - text.count("}")
                if body_index > struct_index and brace_balance <= 0:
                    break
            block_text = "\n".join(block)
            if not re.search(rf"\b{re.escape(name)}\b", block_text):
                candidates.append(
                    {
                        "path": path,
                        "line": index + 1,
                        "struct_line": struct_index + 1,
                        "variable": name,
                        "reason": "local is constructed near a struct literal but not referenced inside it",
                    }
                )
            break
    return candidates


def scan_paths(paths: Iterable[Path]) -> list[dict[str, object]]:
    findings: list[dict[str, object]] = []
    for root in paths:
        if root.is_file() and root.suffix == ".rs":
            findings.extend(scan_source(root.read_text(encoding="utf-8", errors="replace"), str(root)))
        elif root.is_dir():
            for path in root.rglob("*.rs"):
                findings.extend(scan_source(path.read_text(encoding="utf-8", errors="replace"), str(path)))
    return findings


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Report likely unused Rust struct locals")
    parser.add_argument("--scope", action="append", type=Path, default=[])
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    scopes = args.scope or [
        Path("crates/codegen/simplicio-runtime-client"),
        Path("crates/codegen/xai-grok-models"),
        Path("crates/codegen/xai-grok-pager/src/headless.rs"),
        Path("crates/codegen/xai-grok-pager/src/app/cli.rs"),
    ]
    findings = scan_paths(scopes)
    report = {
        "schema": "simplicio.struct-initializer-review/v1",
        "blocking": False,
        "scopes": [str(scope) for scope in scopes],
        "candidate_count": len(findings),
        "candidates": findings,
    }
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else json.dumps(report, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
