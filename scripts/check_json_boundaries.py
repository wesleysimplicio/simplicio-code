#!/usr/bin/env python3
"""Audit source, artifacts and package contents against the JSON inventory.

The scanner is deliberately conservative: a JSON-looking source token, a JSON
file, or a generated/package artifact is a finding until its *exact* path is
reviewed in ``config/json-boundaries.toml``. It emits text by default so the
report itself is not another internal JSON artifact. Use ``--format markdown``
for a publishable human report.
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
SOURCE_SUFFIXES = {".py", ".mjs", ".js", ".ts", ".tsx", ".rs", ".go", ".java", ".cs", ".toml", ".yaml", ".yml"}
ARTIFACT_SUFFIXES = {".json", ".jsonl", ".ndjson"}
BUILD_MANIFESTS = {"Cargo.toml", "Cargo.lock", "package-lock.json", "pnpm-lock.yaml", "yarn.lock"}
NON_CODE_TEXT_SUFFIXES = {".md", ".rst", ".txt", ".lock"}
NON_CODE_FILENAMES = {"THIRD-PARTY-NOTICES", "LICENSE", "README", "NOTICE"}


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
            for field in ("owner", "reason", "expires", "category", "target_format", "status", "producer", "consumer", "lifecycle"):
                if not entry.get(field):
                    raise ValueError(f"{name}: missing {field}")
            try:
                dt.date.fromisoformat(entry["expires"])
            except ValueError as exc:
                raise ValueError(f"{name}: invalid expires date") from exc
            result[name] = entry
    return result


def findings(root: pathlib.Path, include_generated: bool = False) -> list[tuple[str, int, str]]:
    out: list[tuple[str, int, str]] = []
    for path in root.rglob("*"):
        rel_path = path.relative_to(root)
        skipped_generated = any(part in {"target", "dist", "build"} for part in rel_path.parts)
        if (not path.is_file() or any(part in (SKIP - {"target", "dist", "build"}) for part in rel_path.parts)
                or (skipped_generated and not include_generated)
                or rel_path.as_posix() == "scripts/check_json_boundaries.py"):
            continue
        is_artifact = path.suffix.lower() in ARTIFACT_SUFFIXES
        is_source = path.suffix.lower() in SOURCE_SUFFIXES
        if path.name in BUILD_MANIFESTS and not is_artifact:
            continue
        if path.name in NON_CODE_FILENAMES:
            continue
        if not is_source and path.suffix.lower() in NON_CODE_TEXT_SUFFIXES:
            continue
        if is_artifact:
            out.append((rel_path.as_posix(), 1, f"artifact:{path.suffix.lower()}"))
            continue
        # Extensionless and renamed text artifacts are part of the audit too;
        # attempting UTF-8 decoding is the portable binary/text discriminator.
        if not is_source:
            try:
                if path.stat().st_size > 4 * 1024 * 1024:
                    continue
            except OSError:
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


def load_scope(path: pathlib.Path) -> set[str]:
    """Load an exact, reviewable newline-delimited audit scope.

    A scope is intentionally not a glob or a JSON document: it is a small
    checked-in list of paths for a bounded migration lane. Rejecting patterns,
    absolute paths, and duplicate entries prevents a lane from silently
    widening or hiding findings.
    """
    paths: set[str] = set()
    for number, raw in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        value = raw.split("#", 1)[0].strip()
        if not value:
            continue
        candidate = pathlib.PurePosixPath(value)
        if candidate.is_absolute() or any(part in {"", ".", ".."} for part in candidate.parts):
            raise ValueError(f"scope line {number}: path must be repository-relative: {value!r}")
        if any(ch in value for ch in "*?[]"):
            raise ValueError(f"scope line {number}: scope path must be exact: {value!r}")
        if value in paths:
            raise ValueError(f"scope line {number}: duplicate path: {value}")
        paths.add(value)
    return paths


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=pathlib.Path, default=pathlib.Path(__file__).parents[1])
    parser.add_argument("--inventory", type=pathlib.Path)
    parser.add_argument("--mode", choices=("baseline", "strict"), default="baseline")
    parser.add_argument("--max-findings", type=int, default=50)
    parser.add_argument("--include-generated", action="store_true", help="include target/dist/build and package output")
    parser.add_argument("--scope-file", type=pathlib.Path,
                        help="audit only exact repository-relative paths from a newline-delimited file")
    parser.add_argument("--format", choices=("text", "markdown"), default="text")
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
    try:
        scope = load_scope(args.scope_file) if args.scope_file else None
    except (OSError, ValueError) as exc:
        print(f"scope error: {exc}", file=sys.stderr)
        return 2
    all_findings = findings(args.root, include_generated=args.include_generated)
    if scope is not None:
        all_findings = [finding for finding in all_findings if finding[0] in scope]
    for path, line, token in all_findings:
        entry = inventory.get(path)
        if entry is None:
            unknown.append((path, line, token))
        elif dt.date.fromisoformat(entry["expires"]) < today:
            expired.append(path)
        elif entry["status"] == "migration_pending":
            pending.append((path, line, token))
    total = len(unknown) + len(pending) + len(expired)
    unknown_paths = len({path for path, _, _ in unknown})
    pending_paths = len({path for path, _, _ in pending})
    expired_paths = len(set(expired))
    if args.format == "markdown":
        print("# JSON boundary audit\n")
        print(f"- Findings: `{total}` occurrences across `{unknown_paths + pending_paths + expired_paths}` paths  ")
        print(f"- Unclassified: `{len(unknown)}` occurrences across `{unknown_paths}` paths  ")
        print(f"- Pending migration: `{len(pending)}` occurrences across `{pending_paths}` paths  ")
        print(f"- Expired: `{len(expired)}` occurrences across `{expired_paths}` paths\n")
        print("| Status | Path | Line | Match |\n|---|---|---:|---|")
        for path, line, token in (unknown + pending)[: max(0, args.max_findings)]:
            status = "unclassified" if (path, line, token) in unknown else "migration_pending"
            print(f"| {status} | `{path}` | {line} | `{token}` |")
        for path in expired[: max(0, args.max_findings)]:
            print(f"| expired | `{path}` |  |  |")
    else:
        scope_label = f" scope_paths={len(scope)}" if scope is not None else ""
        print(
            f"json-boundaries: findings={total} unknown={len(unknown)} pending={len(pending)} "
            f"expired={len(expired)} unknown_paths={unknown_paths} pending_paths={pending_paths}"
            f" expired_paths={expired_paths}{scope_label}"
        )
        for path, line, token in unknown[: max(0, args.max_findings)]:
            print(f"UNCLASSIFIED {path}:{line}: {token}")
        for path, line, token in pending[: max(0, args.max_findings)]:
            print(f"MIGRATION_PENDING {path}:{line}: {token}")
        for path in expired:
            print(f"EXPIRED {path}")
    if args.mode == "strict" and (unknown or pending or expired):
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
