"""Generate and validate the repository's version/status snapshot."""

from __future__ import annotations

import argparse
import datetime as dt
import os
import re
import subprocess
from pathlib import Path


SCHEMA = "simplicio.code-status/v1"


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


def snapshot(root: Path) -> dict[str, object]:
    py = read_version(root / "pyproject.toml", r'^version\s*=\s*"([^"]+)"$')
    rust = read_version(
        root / "crates" / "codegen" / "xai-grok-version" / "Cargo.toml",
        r'^version\s*=\s*"([^"]+)"$',
    )
    readme = read_version(root / "README.md", r"^Versão atual:\s*\*\*([^*]+)\*\*\.")
    versions = {"python": py, "rust": rust, "readme": readme}
    normalized = {key: normalize(value) for key, value in versions.items()}
    source_consistent = len(set(normalized.values())) == 1
    commit = run_git(root, "rev-parse", "HEAD")
    dirty = bool(run_git(root, "status", "--porcelain"))
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
    }


def render(data: dict[str, object]) -> str:
    versions = data["versions"]
    assert isinstance(versions, dict)
    inventory = data["capability_inventory"]
    assert isinstance(inventory, list)
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
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--output", type=Path)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    root = args.root.resolve()
    data = snapshot(root)
    if args.output:
        output = args.output if args.output.is_absolute() else root / args.output
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(render(data), encoding="utf-8")
    print(data)
    return 0 if not args.check or data["source_version_status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
