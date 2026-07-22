#!/usr/bin/env python3
"""Audit direct workspace/process access against a versioned manifest.

This is deliberately a small, dependency-free architectural gate.  It does
not infer that a method named ``RuntimeClient`` is governed: every matching
call site must be classified in the manifest and violations fail closed.
"""

from __future__ import annotations

import argparse
from collections import Counter
import fnmatch
import json
import re
from pathlib import Path
from typing import Any

SCHEMA = "simplicio.workspace-access-manifest/v1"
DEFAULT_PATTERNS = {
    "filesystem": re.compile(r"(?:std::fs|tokio::fs|std::fs::|tokio::fs::)"),
    "process": re.compile(r"(?:Command::new|tokio::process|std::process)"),
    "walk": re.compile(r"(?:WalkBuilder|ripgrep|rg_path\(\))"),
}
CLASSIFICATIONS = {"runtime-governed", "bootstrap-allowlisted", "test-fixture", "generated", "violation"}


def _rule_matches(rule: dict[str, Any], path: str, kind: str, line: str) -> bool:
    return (
        fnmatch.fnmatch(path, str(rule.get("path", "")))
        and str(rule.get("kind", "")) in (kind, "*")
        and (not rule.get("contains") or str(rule["contains"]) in line)
    )


def audit(root: Path, manifest: Path) -> dict[str, Any]:
    spec = json.loads(manifest.read_text(encoding="utf-8"))
    if spec.get("schema") != SCHEMA:
        raise ValueError(f"unsupported manifest schema: {spec.get('schema')!r}")
    rules = spec.get("rules")
    if not isinstance(rules, list):
        raise ValueError("manifest rules must be a list")

    findings: list[dict[str, Any]] = []
    for scope in spec.get("scopes", ["crates/codegen"]):
        scope_path = root / str(scope)
        if not scope_path.exists():
            continue
        for path in sorted(p for p in scope_path.rglob("*") if p.is_file() and p.suffix in {".rs", ".py", ".ts", ".tsx"}):
            rel = path.relative_to(root).as_posix()
            in_cfg_test = False
            for line_number, line in enumerate(path.read_text(encoding="utf-8", errors="replace").splitlines(), 1):
                if re.search(r"^\s*#\[cfg\(test\)\]", line):
                    in_cfg_test = True
                for kind, pattern in DEFAULT_PATTERNS.items():
                    if not pattern.search(line):
                        continue
                    rule = next((r for r in rules if isinstance(r, dict) and _rule_matches(r, rel, kind, line)), None)
                    if rule and not (in_cfg_test and str(rule.get("classification")) == "violation"):
                        classification = str(rule.get("classification"))
                    elif in_cfg_test:
                        classification = "test-fixture"
                        rule = {
                            "classification": "test-fixture",
                            "owner": "test-suite",
                            "rationale": "Access is contained in a cfg(test) module and is not a productive workspace path.",
                        }
                    else:
                        classification = "violation"
                    if classification not in CLASSIFICATIONS:
                        classification = "violation"
                    findings.append({
                        "path": rel,
                        "line": line_number,
                        "kind": kind,
                        "classification": classification,
                        "owner": rule.get("owner") if rule else None,
                        "rationale": rule.get("rationale") if rule else None,
                    })

    violations = [f for f in findings if f["classification"] == "violation"]
    unclassified = [f for f in findings if f["owner"] is None]
    observed = Counter((f["path"], f["kind"], f["classification"]) for f in findings)
    baseline = spec.get("baseline", [])
    baseline_errors: list[dict[str, Any]] = []
    if not isinstance(baseline, list):
        raise ValueError("manifest baseline must be a list")
    allowed: dict[tuple[str, str, str], int] = {}
    for entry in baseline:
        if not isinstance(entry, dict):
            raise ValueError("manifest baseline entries must be objects")
        try:
            key = (str(entry["path"]), str(entry["kind"]), str(entry["classification"]))
            count = int(entry["max_count"])
        except (KeyError, TypeError, ValueError) as exc:
            raise ValueError("baseline entries require path, kind, classification, and non-negative max_count") from exc
        if count < 0 or key in allowed:
            raise ValueError("baseline entries must have unique keys and non-negative max_count")
        allowed[key] = count
    if baseline:
        for key, count in sorted(observed.items()):
            maximum = allowed.get(key)
            if maximum is None or count > maximum:
                baseline_errors.append({
                    "path": key[0], "kind": key[1], "classification": key[2],
                    "observed": count, "max_count": maximum,
                })
    return {
        "schema": SCHEMA,
        "status": "failed" if violations or unclassified or baseline_errors else "passed",
        "findings": findings,
        "violations": violations,
        "unclassified": unclassified,
        "baseline_errors": baseline_errors,
        "summary": {"total": len(findings), "violations": len(violations), "unclassified": len(unclassified), "baseline_errors": len(baseline_errors)},
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", default=".")
    parser.add_argument("--manifest", default="docs/contracts/workspace-access-manifest.json")
    args = parser.parse_args(argv)
    try:
        result = audit(Path(args.root).resolve(), Path(args.root).resolve() / args.manifest)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        result = {"schema": SCHEMA, "status": "error", "errors": [str(exc)]}
    print(json.dumps(result, ensure_ascii=False, indent=2, sort_keys=True))
    return 0 if result.get("status") == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
