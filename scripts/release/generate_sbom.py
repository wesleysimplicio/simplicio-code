#!/usr/bin/env python3
"""Generate a minimal CycloneDX-shaped SBOM for the simplicio-code binary
crate from `cargo metadata`.

This is a deliberately small, dependency-free (stdlib only) substitute for
a full SBOM tool such as `cargo-cyclonedx` or `syft`, which are not
available in this environment/session. It records every resolved crate in
the dependency graph reachable from the given root package: name, version,
and the source it was resolved from (crates.io / git / path).

Usage:
    python3 scripts/release/generate_sbom.py \
        --root-package xai-grok-pager-bin \
        --version 0.3.0-beta.1 \
        --out sbom.cdx.json

Exit codes: 0 on success, 1 if `cargo metadata` fails or the root package
isn't found in the resolved graph.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from datetime import datetime, timezone


def run_cargo_metadata(manifest_dir: str) -> dict:
    proc = subprocess.run(
        ["cargo", "metadata", "--format-version=1", "--locked"],
        cwd=manifest_dir,
        capture_output=True,
        text=True,
        encoding="utf-8",
        errors="replace",
    )
    if proc.returncode != 0:
        sys.stderr.write("cargo metadata failed:\n" + proc.stderr + "\n")
        sys.exit(1)
    return json.loads(proc.stdout)


def purl_for(name: str, version: str, source: str | None) -> str:
    if source and source.startswith("git+"):
        return f"pkg:cargo/{name}@{version}?vcs_url={source}"
    # Default: assume crates.io (cargo's implicit registry) for anything
    # without an explicit non-registry source.
    return f"pkg:cargo/{name}@{version}"


def build_sbom(metadata: dict, root_package: str, version: str) -> dict:
    packages = {p["id"]: p for p in metadata["packages"]}
    root_id = next(
        (pid for pid, p in packages.items() if p["name"] == root_package),
        None,
    )
    if root_id is None:
        sys.stderr.write(f"root package '{root_package}' not found in cargo metadata output\n")
        sys.exit(1)

    resolve = metadata["resolve"]
    nodes = {n["id"]: n for n in resolve["nodes"]}

    # BFS over the dependency graph reachable from the root package so the
    # SBOM reflects what's actually linked into the shipped binary, not
    # every crate in the workspace.
    seen = set()
    queue = [root_id]
    while queue:
        pid = queue.pop()
        if pid in seen:
            continue
        seen.add(pid)
        node = nodes.get(pid)
        if not node:
            continue
        for dep in node.get("deps", []):
            if dep["pkg"] not in seen:
                queue.append(dep["pkg"])

    components = []
    for pid in sorted(seen):
        pkg = packages.get(pid)
        if not pkg or pkg["id"] == root_id:
            continue
        components.append(
            {
                "type": "library",
                "name": pkg["name"],
                "version": pkg["version"],
                "purl": purl_for(pkg["name"], pkg["version"], pkg.get("source")),
                "licenses": (
                    [{"license": {"id": pkg["license"]}}] if pkg.get("license") else []
                ),
            }
        )

    sbom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "serialNumber": None,
        "version": 1,
        "metadata": {
            "timestamp": datetime.now(timezone.utc).isoformat(),
            "component": {
                "type": "application",
                "name": root_package,
                "version": version,
            },
        },
        "components": components,
    }
    return sbom


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root-package", default="xai-grok-pager-bin")
    parser.add_argument("--version", required=True)
    parser.add_argument("--manifest-dir", default=".")
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    metadata = run_cargo_metadata(args.manifest_dir)
    sbom = build_sbom(metadata, args.root_package, args.version)

    with open(args.out, "w", encoding="utf-8") as f:
        json.dump(sbom, f, indent=2)
        f.write("\n")

    print(f"Wrote SBOM with {len(sbom['components'])} components to {args.out}")


if __name__ == "__main__":
    main()
