#!/usr/bin/env python3
"""Hermetic tests for the GitHub repository preflight (#59)."""
from __future__ import annotations

import json
import os
import sys
import tempfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[1]
sys.path.insert(0, str(REPO / "scripts"))

import resolve_github_repo as resolver  # noqa: E402


def main() -> int:
    checks: list[bool] = []

    def check(label: str, condition: bool) -> None:
        checks.append(condition)
        print(f"  [{'ok' if condition else 'XX'}] {label}")

    check(
        "HTTPS remote is normalized",
        resolver.repository_from_remote("https://github.com/wesleysimplicio/simplicio-code.git")
        == "wesleysimplicio/simplicio-code",
    )
    check(
        "SSH remote is normalized",
        resolver.repository_from_remote("git@github.com:wesleysimplicio/simplicio-code.git")
        == "wesleysimplicio/simplicio-code",
    )
    check("non-GitHub remote is rejected", resolver.repository_from_remote("https://example.com/a/b") is None)

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        result = resolver.resolve(
            root,
            remote="https://github.com/wesleysimplicio/grok-build.git",
        )
        check("wrong origin fails closed", result.status == "remote_mismatch" and not result.ok)

        result = resolver.resolve(
            root,
            explicit="wesleysimplicio/simplicio-code",
            remote="https://github.com/wesleysimplicio/grok-build.git",
        )
        check("--repo is an explicit, auditable override", result.status == "explicit_override" and result.ok)

        contract_dir = root / ".simplicio"
        contract_dir.mkdir()
        (contract_dir / "repository.json").write_text(
            json.dumps({"github": {"repository": "acme/product"}}), encoding="utf-8"
        )
        result = resolver.resolve(root, remote="git@github.com:acme/product.git")
        check("workspace contract is honored", result.status == "ready" and result.repository == "acme/product")

    previous = os.environ.get("SIMPLICIO_GITHUB_REPOSITORY")
    os.environ["SIMPLICIO_GITHUB_REPOSITORY"] = "acme/from-env"
    try:
        with tempfile.TemporaryDirectory() as directory:
            result = resolver.resolve(Path(directory), remote="https://github.com/acme/from-env")
            check("environment contract is honored", result.status == "ready")
    finally:
        if previous is None:
            os.environ.pop("SIMPLICIO_GITHUB_REPOSITORY", None)
        else:
            os.environ["SIMPLICIO_GITHUB_REPOSITORY"] = previous

    print(f"selftest: {'PASS' if all(checks) else 'FAIL'} ({sum(checks)}/{len(checks)})")
    return 0 if all(checks) else 1


if __name__ == "__main__":
    raise SystemExit(main())
