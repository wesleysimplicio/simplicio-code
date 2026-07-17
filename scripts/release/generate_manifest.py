#!/usr/bin/env python3
"""Build a signed-release manifest (`manifest.json`) for simplicio-code.

Scans a directory of built artifacts named
`simplicio-code-<version>-<platform>[.exe]`, computes each file's SHA-256,
and writes a `ReleaseManifest` JSON document matching the schema consumed
by `crates/codegen/xai-grok-update/src/manifest_verify.rs`
(`ReleaseManifest { version, channel, artifacts: [{ platform, filename,
sha256 }] }`).

This manifest is the thing that later gets Ed25519-signed (see
`sign_manifest.sh`) — signing happens over these exact bytes, so this
script's output must be treated as final once written (don't hand-edit a
signed manifest).

Usage:
    python3 scripts/release/generate_manifest.py \
        --version 0.3.0-beta.1 --channel beta \
        --artifacts-dir dist/ --out dist/manifest.json
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
from pathlib import Path

ARTIFACT_RE = re.compile(r"^simplicio-code-(?P<version>.+)-(?P<platform>[a-z0-9_]+-[a-z0-9_]+)(\.exe)?$")


def sha256_of(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--version", required=True)
    parser.add_argument("--channel", required=True, choices=["beta", "stable"])
    parser.add_argument("--artifacts-dir", required=True)
    parser.add_argument("--out", required=True)
    args = parser.parse_args()

    artifacts_dir = Path(args.artifacts_dir)
    entries = []
    for path in sorted(artifacts_dir.iterdir()):
        if not path.is_file():
            continue
        m = ARTIFACT_RE.match(path.name)
        if not m or m.group("version") != args.version:
            continue
        entries.append(
            {
                "platform": m.group("platform"),
                "filename": path.name,
                "sha256": sha256_of(path),
            }
        )

    if not entries:
        sys.stderr.write(
            f"no artifacts matching 'simplicio-code-{args.version}-<platform>' found in {artifacts_dir}\n"
        )
        sys.exit(1)

    manifest = {
        "version": args.version,
        "channel": args.channel,
        "artifacts": entries,
    }

    out_path = Path(args.out)
    # Stable, canonical formatting: sorted keys, no ambiguous whitespace, so
    # re-running this script against the same inputs reproduces byte-
    # identical output (needed for a reproducible signature).
    out_path.write_text(json.dumps(manifest, sort_keys=True, indent=2) + "\n", encoding="utf-8")
    print(f"Wrote manifest with {len(entries)} artifact(s) to {out_path}")


if __name__ == "__main__":
    main()
