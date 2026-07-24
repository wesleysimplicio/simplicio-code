#!/usr/bin/env python3
"""Standalone no-internal-JSON scanner for Python repositories.

It mirrors the Runtime crate's v1 policy contract using only Python's standard
library. Output is Markdown plus one HBP evidence row; no JSON report exists.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import os
import re
import tomllib
from pathlib import Path, PurePosixPath

PATTERNS = (
    ("serialization-library", "serde_json"),
    ("serialization-library", "orjson"),
    ("serialization-call", "json.dumps"),
    ("serialization-call", "json.loads"),
    ("serialization-call", "JSON.parse"),
    ("serialization-call", "JSON.stringify"),
    ("protocol", "JSON-RPC"),
    ("protocol", "json-rpc"),
)
IGNORED = {".git", "target", "node_modules", "vendor", ".venv", ".simplicio"}
DATE = re.compile(r"^\d{4}-\d{2}-\d{2}$")
BOUNDARY_STATUSES = {"exception", "migration_pending", "migrated"}


def load_policy(path: Path, today: str) -> dict:
    policy = tomllib.loads(path.read_text(encoding="utf-8"))
    if policy.get("schema") != "simplicio.no-internal-json/v1" or policy.get("version") != 1:
        raise ValueError("unsupported policy schema/version")
    if not policy.get("scanner_version"):
        raise ValueError("scanner_version is required")
    seen = set()
    for exception in policy.get("exceptions", []):
        required = ("path", "category", "owner", "external_dependency", "justification", "review_date", "removal_date")
        if any(not exception.get(key) for key in required):
            raise ValueError("exception has a missing required field")
        path_value = exception["path"]
        if path_value in seen or path_value.startswith("/") or ".." in Path(path_value).parts or any(c in path_value for c in "*?[]"):
            raise ValueError(f"exception path is not exact: {path_value}")
        seen.add(path_value)
        if not DATE.fullmatch(exception["review_date"]) or not DATE.fullmatch(exception["removal_date"]):
            raise ValueError(f"invalid exception dates: {path_value}")
        if exception["removal_date"] < today:
            raise ValueError(f"expired exception: {path_value}")
    policy["scan_date"] = today
    return policy


def load_boundaries(path: Path) -> dict[str, dict]:
    """Load the exact path classifications from the repository inventory."""
    raw = tomllib.loads(path.read_text(encoding="utf-8"))
    if raw.get("schema") != "simplicio.json-boundaries/v1":
        raise ValueError("unsupported JSON-boundaries schema")

    entries: dict[str, dict] = {}
    groups = [*raw.get("boundary", []), *raw.get("audit", [])]
    for group in groups:
        names = group.get("paths") or ([group["path"]] if group.get("path") else [])
        target = group.get("target_format", "")
        status = group.get("status") or (
            "exception"
            if target.lower().startswith(("preserve", "json-rpc", "external", "signed", "cyclonedx"))
            else "migration_pending"
        )
        entry = {
            **group,
            "status": status,
            "reason": group.get("reason") or group.get("rationale") or group.get("lifecycle", ""),
            "expires": group.get("expires") or ("2099-12-31" if status == "exception" else "2026-12-31"),
        }
        if status not in BOUNDARY_STATUSES:
            raise ValueError(f"invalid boundary status: {status}")
        for name in names:
            candidate = PurePosixPath(name)
            if (
                not name
                or candidate.is_absolute()
                or any(part in {"", ".", ".."} for part in candidate.parts)
                or candidate.as_posix() != name
                or any(char in name for char in "*?[]")
            ):
                raise ValueError(f"boundary path is not exact: {name!r}")
            if name in entries:
                raise ValueError(f"duplicate boundary path: {name}")
            for field in ("owner", "reason", "expires", "category", "target_format", "status", "producer", "consumer", "lifecycle"):
                if not entry.get(field):
                    raise ValueError(f"{name}: missing {field}")
            if not DATE.fullmatch(entry["expires"]):
                raise ValueError(f"invalid boundary expiry date: {name}")
            entries[name] = entry
    return entries


def _classification(
    relative: str,
    policy: dict,
    boundaries: dict[str, dict] | None,
    today: str,
) -> tuple[str, str]:
    """Return (category, state), preserving unknown and pending separately."""
    if boundaries is not None:
        entry = boundaries.get(relative)
        if entry is None:
            return "unclassified", "unknown"
        if entry["expires"] < today:
            return entry["category"], "expired"
        if entry["status"] == "migration_pending":
            return entry["category"], "pending"
        return entry["category"], "classified"

    for exception in policy.get("exceptions", []):
        if exception["path"] == relative:
            return exception["category"], "classified"
    return "unclassified", "unknown"


def scan(
    root: Path,
    policy: dict,
    boundaries: dict[str, dict] | None = None,
) -> list[tuple[str, int, str, str, str]]:
    today = policy.get("scan_date", "9999-12-31")
    findings = []
    for directory, names, files in os.walk(root):
        names[:] = sorted(name for name in names if name not in IGNORED and not name.startswith("."))
        for name in sorted(files):
            path = Path(directory) / name
            relative = path.relative_to(root).as_posix()
            category, _ = _classification(relative, policy, boundaries, today)
            suffix = path.suffix.lower()
            if suffix in {".json", ".jsonl", ".ndjson"}:
                findings.append((relative, 1, "artifact-extension", suffix[1:], category))
            try:
                data = path.read_bytes()
            except OSError:
                continue
            if len(data) > 4 * 1024 * 1024 or b"\0" in data:
                continue
            try:
                text = data.decode("utf-8")
            except UnicodeDecodeError:
                continue
            if suffix not in {".json", ".jsonl", ".ndjson"} and text.strip().startswith("{") and text.strip().endswith("}"):
                findings.append((relative, 1, "renamed-json-artifact", "object-document", category))
            for line_number, line in enumerate(text.splitlines(), 1):
                for kind, needle in PATTERNS:
                    if needle in line:
                        findings.append((relative, line_number, kind, needle, category))
    return sorted(set(findings))


def render(
    findings: list[tuple[str, int, str, str, str]],
    policy: dict,
    mode: str,
    boundaries: dict[str, dict] | None = None,
) -> tuple[str, str, int]:
    today = policy.get("scan_date", "9999-12-31")
    states = [
        _classification(path, policy, boundaries, today)[1]
        for path, _, _, _, _ in findings
    ]
    unknown = states.count("unknown")
    pending = states.count("pending")
    expired = states.count("expired")
    blocking = unknown + pending + expired
    status = "FAIL" if mode == "strict" and blocking else ("UNVERIFIED" if blocking else "PASS")
    lines = [
        "# No-internal-JSON policy scan",
        "",
        f"- status: `{status}`",
        f"- mode: `{mode}`",
        f"- scanner_version: `{policy['scanner_version']}`",
        f"- findings: `{len(findings)}`",
        f"- unknown: `{unknown}`",
        f"- unclassified: `{unknown}`",
        f"- pending: `{pending}`",
        f"- expired: `{expired}`",
        "",
        "## Findings",
        "",
        "| Path | Line | Kind | Classification | State | Evidence |",
        "| --- | ---: | --- | --- | --- | --- |",
    ]
    lines.extend(
        f"| `{path}` | {line} | `{kind}` | `{category}` | `{_classification(path, policy, boundaries, today)[1]}` | `{evidence}` |"
        for path, line, kind, evidence, category in findings
    )
    markdown = "\n".join(lines) + "\n"
    payload = f"mode={mode};status={status};policy_version={policy['version']};scanner_version={policy['scanner_version']};findings={len(findings)};unknown={unknown};pending={pending};expired={expired}"
    fields = ("0", "genesis", "policy-scan", payload, "policy-scanner:" + policy["scanner_version"])
    digest = hashlib.sha256(b"".join(len(field).to_bytes(8, "little") + field.encode() for field in fields) + (0).to_bytes(8, "little")).hexdigest()
    hbp = f"schema=simplicio.hbp/v1\nversion=1.0.0\nseq=0\nprev_hash=genesis\ntopic=policy-scan\npayload={payload}\nprovenance={fields[-1]}\nhash={digest}\n"
    return markdown, hbp, 1 if status == "FAIL" else 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path("."))
    parser.add_argument("--policy", type=Path)
    parser.add_argument("--boundaries", type=Path)
    parser.add_argument("--mode", choices=("baseline", "strict"), default="baseline")
    parser.add_argument("--markdown", type=Path)
    parser.add_argument("--hbp", type=Path)
    args = parser.parse_args()
    # Use the real UTC date by default. A far-future default made every
    # realistically dated exception look expired and divorced CI results from
    # the policy's actual review clock.
    today = os.environ.get(
        "SIMPLICIO_POLICY_SCAN_DATE",
        datetime.datetime.now(datetime.UTC).date().isoformat(),
    )
    policy_path = args.policy or args.repo / "policy/no-internal-json.toml"
    boundaries_path = args.boundaries or args.repo / "config/json-boundaries.toml"
    policy = load_policy(policy_path, today)
    boundaries = load_boundaries(boundaries_path) if boundaries_path.is_file() else None
    markdown, hbp, code = render(scan(args.repo, policy, boundaries), policy, args.mode, boundaries)
    (args.markdown.write_text(markdown, encoding="utf-8") if args.markdown else print(markdown, end=""))
    if args.hbp:
        args.hbp.write_text(hbp, encoding="utf-8")
    return code


if __name__ == "__main__":
    raise SystemExit(main())
