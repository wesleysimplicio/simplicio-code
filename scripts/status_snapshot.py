"""Generate and validate the repository's version/status snapshot."""

from __future__ import annotations

import argparse
import datetime as dt
import json
import os
import re
import subprocess
import sys
from pathlib import Path


SCHEMA = "simplicio.code-status/v1"
RESIDUAL_SCHEMA = "simplicio.code-residual-issues/v1"
RESIDUAL_PATH = Path("docs/status/residual-issues.v1.json")


def capability_inventory(root: Path) -> list[dict[str, str]]:
    runtime_client = (
        root / "crates" / "codegen" / "simplicio-runtime-client" / "src" / "lib.rs"
    ).read_text(encoding="utf-8")
    loop_hub = (
        root / "crates" / "codegen" / "simplicio-runtime-client" / "src" / "loop_hub.rs"
    ).read_text(encoding="utf-8")
    agent_client = (
        root / "crates" / "codegen" / "simplicio-agent-client" / "src" / "lib.rs"
    ).read_text(encoding="utf-8")
    return [
        {
            "capability": "workspace-access-audit",
            "source_status": "implemented" if (root / "scripts" / "audit_workspace_access.py").exists() else "missing",
            "external_evidence": "local-audit-required",
        },
        {
            "capability": "runtime-exec/v1",
            "source_status": "implemented" if "simplicio_exec" in runtime_client else "missing",
            "external_evidence": "installed-runtime-required",
        },
        {
            "capability": "runtime-process-lifecycle/v1",
            "source_status": "implemented" if "LOOP_HUB_RUNTIME_PROCESS_SCHEMA" in loop_hub else "missing",
            "external_evidence": "installed-runtime-hub-required",
        },
        {
            "capability": "agent-host/v1",
            "source_status": "implemented" if "HOST_PROTOCOL_SCHEMA" in agent_client else "missing",
            "external_evidence": "installed-agent-host-required",
        },
        {
            "capability": "workspace.observe/v1",
            "source_status": "implemented" if "WORKSPACE_OBSERVE_SCHEMA" in agent_client else "missing",
            "external_evidence": "agent-host-emission-required",
        },
        {
            "capability": "simplicio-gateway/v1",
            "source_status": (
                "contract-client-only"
                if (root / "crates" / "codegen" / "simplicio-code-gateway").exists()
                else "missing"
            ),
            "external_evidence": "production-wiring-and-backend-required",
        },
        {
            "capability": "simplicio-account-device-auth/v1",
            "source_status": (
                "contract-client-only"
                if (root / "crates" / "codegen" / "simplicio-account-client").exists()
                else "missing"
            ),
            "external_evidence": "production-wiring-and-backend-required",
        },
    ]


def run_git(root: Path, *args: str) -> str:
    result = subprocess.run(
        ["git", *args],
        cwd=root,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        text=True,
        check=True,
        # Python 3.14 on Windows can hand an invalid pytest capture handle
        # to CreateProcess when close_fds is left at its default.
        close_fds=False if os.name == "nt" else True,
    )
    return result.stdout.strip()


def read_version(path: Path, pattern: str) -> str:
    match = re.search(pattern, path.read_text(encoding="utf-8"), re.MULTILINE)
    if not match:
        raise ValueError(f"version not found in {path}")
    return match.group(1)


def normalize(version: str) -> str:
    return version.replace("-beta.", "b").replace("-beta", "b")


def read_bundle_code_version(path: Path) -> str:
    payload = json.loads(path.read_text(encoding="utf-8"))
    for component in payload.get("components", []):
        if isinstance(component, dict) and component.get("name") == "code":
            version = component.get("version")
            if isinstance(version, str) and version:
                return version
    raise ValueError(f"code version not found in {path}")


def load_residual_inventory(root: Path) -> dict[str, object]:
    path = root / RESIDUAL_PATH
    payload = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(payload, dict) or payload.get("schema") != RESIDUAL_SCHEMA:
        raise ValueError(f"invalid residual inventory schema in {path}")
    issues = payload.get("issues")
    if not isinstance(issues, list) or not issues:
        raise ValueError(f"residual inventory has no issues in {path}")
    allowed_states = {
        "OPEN",
        "IN_PROGRESS",
        "BLOCKED",
        "MERGED_PENDING_CLOSURE",
        "NOT_EXECUTED",
        "UNKNOWN",
        "HISTORICAL_CLOSED",
        "IMPLEMENTED",
        "VERIFIED",
    }
    for item in issues:
        if not isinstance(item, dict):
            raise ValueError(f"invalid residual issue entry in {path}")
        number = item.get("number")
        if not isinstance(number, int) or number <= 0:
            raise ValueError(f"invalid residual issue number in {path}")
        if item.get("state") not in allowed_states:
            raise ValueError(f"invalid residual issue state for #{number}")
        if not isinstance(item.get("dependencies"), list) or not item.get("evidence"):
            raise ValueError(f"residual issue #{number} lacks dependencies/evidence")
    for key in ("source", "source_revision", "captured_at_utc"):
        if not payload.get(key):
            raise ValueError(f"residual inventory lacks {key}")
    return payload


def dirty_checkout(root: Path) -> bool:
    entries = []
    for line in run_git(root, "status", "--porcelain").splitlines():
        path = line[3:] if len(line) > 3 else ""
        if (
            path == ".simplicio-lease.json"
            or path.startswith(".simplicio/")
            or path == "docs/status/current.md"
        ):
            continue
        entries.append(line)
    return bool(entries)


def _is_ancestor(root: Path, commit: str) -> bool:
    result = subprocess.run(
        ["git", "merge-base", "--is-ancestor", commit, "HEAD"],
        cwd=root,
        stdin=subprocess.DEVNULL,
        capture_output=True,
        check=False,
        close_fds=False if os.name == "nt" else True,
    )
    return result.returncode == 0


def snapshot(root: Path) -> dict[str, object]:
    py = read_version(root / "pyproject.toml", r'^version\s*=\s*"([^"]+)"$')
    rust = read_version(
        root / "crates" / "codegen" / "xai-grok-version" / "Cargo.toml",
        r'^version\s*=\s*"([^"]+)"$',
    )
    readme = read_version(root / "README.md", r"^Versão atual:\s*\*\*([^*]+)\*\*\.")
    bundle_code = read_bundle_code_version(root / "config" / "onboarding-bundle-v1.json")
    versions = {"python": py, "rust": rust, "readme": readme, "onboarding_bundle": bundle_code}
    normalized = {key: normalize(value) for key, value in versions.items()}
    source_consistent = len(set(normalized.values())) == 1
    commit = run_git(root, "rev-parse", "HEAD")
    dirty = dirty_checkout(root)
    try:
        exact_tag = run_git(root, "describe", "--tags", "--exact-match", "HEAD")
    except subprocess.CalledProcessError:
        exact_tag = None
    release_status = "PASS" if exact_tag and not dirty else "UNKNOWN"
    return {
        "schema": SCHEMA,
        "generated_at_utc": dt.datetime.now(dt.timezone.utc).isoformat(),
        "commit": commit,
        "dirty": dirty,
        "versions": versions,
        "normalized_versions": normalized,
        "source_version_status": "PASS" if source_consistent else "FAIL",
        "release_evidence_status": release_status,
        "exact_tag": exact_tag,
        "release_evidence_reason": (
            "clean checkout at an exact tag"
            if release_status == "PASS"
            else "release provenance is not proven by this checkout"
        ),
        "capability_inventory": capability_inventory(root),
        "residual_inventory": load_residual_inventory(root),
    }


def render(data: dict[str, object]) -> str:
    versions = data["versions"]
    assert isinstance(versions, dict)
    inventory = data["capability_inventory"]
    assert isinstance(inventory, list)
    residual = data["residual_inventory"]
    assert isinstance(residual, dict)
    issues = residual["issues"]
    assert isinstance(issues, list)
    lines = [
        "# Current status",
        "",
        f"- Schema: `{data['schema']}`",
        f"- Generated: `{data['generated_at_utc']}`",
        f"- Commit: `{data['commit']}`",
        f"- Dirty checkout: `{data['dirty']}`",
        "",
        "## Version sources",
        "",
        "| Source | Version |",
        "| --- | --- |",
    ]
    lines.extend(f"| {key} | `{value}` |" for key, value in versions.items())
    lines.extend(
        [
            "",
            f"- Source version status: `{data['source_version_status']}`",
            f"- Release evidence status: `{data['release_evidence_status']}`",
            f"- Exact tag: `{data['exact_tag'] or 'null'}`",
            f"- Release evidence reason: {data['release_evidence_reason']}",
            "",
            "This file is evidence for the current checkout, not proof of a published release.",
            "",
            "## Capability inventory",
            "",
            "Derived from source presence; external evidence is listed separately and is not inferred as PASS.",
            "",
            "| Capability | Source status | External evidence |",
            "| --- | --- | --- |",
        ]
    )
    lines.extend(
        f"| {item['capability']} | `{item['source_status']}` | `{item['external_evidence']}` |"
        for item in inventory
    )
    lines.extend(
        [
            "",
            "## Residual issue inventory",
            "",
            f"- Source: {residual['source']}",
            f"- Source revision: `{residual['source_revision']}`",
            f"- Captured: `{residual['captured_at_utc']}`",
            "",
            "Statuses are evidence states, not closure claims; HISTORICAL_CLOSED preserves GitHub history without asserting current acceptance; refresh this inventory after live GitHub re-query.",
            "",
            "| Issue | State | Priority | Owner | Dependencies | Evidence |",
            "| --- | --- | --- | --- | --- | --- |",
        ]
    )
    lines.extend(
        f"| #{item['number']} | `{item['state']}` | {item.get('priority', 'UNSET')} | {item.get('owner', 'UNSET')} | {'; '.join(item['dependencies'])} | {item['evidence']} |"
        for item in issues
    )
    lines.extend(
        [
            "",
            "## Migration note",
            "",
            "Current onboarding pins and their measured drift are documented in [`docs/migration/code-status-beta5.md`](../migration/code-status-beta5.md).",
            "",
        ]
    )
    return "\n".join(lines)


def validate_rendered_document(root: Path, data: dict[str, object]) -> None:
    path = root / "docs/status/current.md"
    if not path.exists():
        raise ValueError(f"missing generated status document: {path}")
    text = path.read_text(encoding="utf-8")
    commit_match = re.search(r"^- Commit: `([0-9a-f]{40})`$", text, re.MULTILINE)
    if not commit_match or not _is_ancestor(root, commit_match.group(1)):
        raise ValueError("status document commit is missing or not an ancestor of checkout")
    dirty_match = re.search(r"^- Dirty checkout: `(True|False)`$", text, re.MULTILINE)
    if not dirty_match or dirty_match.group(1) != str(data["dirty"]):
        raise ValueError("status document dirty flag drifted")
    versions = data["versions"]
    assert isinstance(versions, dict)
    for key, value in versions.items():
        if f"| {key} | `{value}` |" not in text:
            raise ValueError(f"status document is missing version source {key}")
    if f"- Source version status: `{data['source_version_status']}`" not in text:
        raise ValueError("status document source version status drifted")
    inventory = data["capability_inventory"]
    assert isinstance(inventory, list)
    for item in inventory:
        row = f"| {item['capability']} | `{item['source_status']}` | `{item['external_evidence']}` |"
        if row not in text:
            raise ValueError(f"status document is missing capability {item['capability']}")
    residual = data["residual_inventory"]
    assert isinstance(residual, dict)
    source_revision_match = re.search(r"^- Source revision: `([^`]+)`$", text, re.MULTILINE)
    if not source_revision_match or source_revision_match.group(1) != residual.get("source_revision"):
        raise ValueError("status document source revision drifted")
    for item in residual["issues"]:
        row_prefix = f"| #{item['number']} | `{item['state']}` |"
        if row_prefix not in text:
            raise ValueError(f"status document is missing residual issue #{item['number']}")
    migration_note = root / "docs/migration/code-status-beta5.md"
    if not migration_note.exists() or "code-status-beta5.md" not in text:
        raise ValueError("status document is missing the beta.5 migration note")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    data = snapshot(root)
    if hasattr(sys.stdout, "reconfigure"):
        sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    if args.output:
        output = args.output if args.output.is_absolute() else root / args.output
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(render(data), encoding="utf-8")
    print(data)
    if not args.check:
        return 0
    if data["source_version_status"] != "PASS":
        return 1
    try:
        validate_rendered_document(root, data)
    except ValueError as exc:
        print(f"status check failed: {exc}")
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
