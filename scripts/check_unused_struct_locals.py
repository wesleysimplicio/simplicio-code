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


def candidate_key(candidate: dict[str, object]) -> tuple[str, int, int, str]:
    return (
        Path(str(candidate["path"])).as_posix(),
        int(candidate["line"]),
        int(candidate["struct_line"]),
        str(candidate["variable"]),
    )


def load_allowlist(path: Path) -> set[tuple[str, int, int, str]]:
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or payload.get("schema") != "simplicio.struct-initializer-allowlist/v1":
        raise ValueError(f"invalid allowlist schema in {path}")
    entries = payload.get("entries")
    if not isinstance(entries, list):
        raise ValueError(f"allowlist entries must be a list in {path}")
    allowlisted = {candidate_key(entry) for entry in entries if isinstance(entry, dict)}
    if len(allowlisted) != len(entries):
        raise ValueError(f"allowlist contains invalid or duplicate entries in {path}")
    return allowlisted


def evaluate(
    findings: list[dict[str, object]],
    allowlisted: set[tuple[str, int, int, str]],
    enforce: bool,
) -> dict[str, object]:
    finding_keys = {candidate_key(item) for item in findings}
    unexpected = [item for item in findings if candidate_key(item) not in allowlisted]
    stale = sorted(allowlisted - finding_keys)
    status = (
        "PASS" if not unexpected and not stale else "FAIL"
    ) if enforce else ("REVIEW_ONLY" if findings else "PASS")
    return {
        "status": status,
        "blocking": enforce,
        "allowlisted_count": len(findings) - len(unexpected),
        "unallowlisted_count": len(unexpected),
        "stale_allowlist_count": len(stale),
        "unallowlisted_candidates": unexpected,
        "stale_allowlist": [list(item) for item in stale],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Report likely unused Rust struct locals")
    parser.add_argument("--scope", action="append", type=Path, default=[])
    parser.add_argument("--allowlist", type=Path, help="exact candidate identities allowed by review")
    parser.add_argument("--enforce", action="store_true", help="fail on unallowlisted or stale candidates")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    scopes = args.scope or [
        Path("crates/codegen/simplicio-runtime-client"),
        Path("crates/codegen/xai-grok-models"),
        Path("crates/codegen/xai-grok-pager/src/headless.rs"),
        Path("crates/codegen/xai-grok-pager/src/app/cli.rs"),
    ]
    findings = scan_paths(scopes)
    allowlisted = load_allowlist(args.allowlist) if args.allowlist else set()
    evaluation = evaluate(findings, allowlisted, args.enforce)
    report = {
        "schema": "simplicio.struct-initializer-review/v1",
        "scopes": [str(scope) for scope in scopes],
        "candidate_count": len(findings),
        "candidates": findings,
        **evaluation,
    }
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else json.dumps(report, indent=2))
    return 1 if args.enforce and report["status"] != "PASS" else 0


if __name__ == "__main__":
    raise SystemExit(main())
