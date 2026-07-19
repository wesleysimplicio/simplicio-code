#!/usr/bin/env python3
"""Resolve the GitHub repository for workspace-scoped automation.

Never let a GitHub command infer its target from an unrelated ``origin``.
The resolver prefers an explicit CLI value, then ``SIMPLICIO_GITHUB_REPOSITORY``,
then ``.simplicio/repository.json`` and finally the canonical repository for
this product.  A conflicting remote is reported instead of being silently
used.
"""
from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
from dataclasses import dataclass
from pathlib import Path
from urllib.parse import urlparse

CANONICAL_REPOSITORY = "wesleysimplicio/simplicio-code"
REPOSITORY_PATTERN = re.compile(r"^[^/\s]+/[^/\s]+$")


def normalize_repository(value: str) -> str:
    """Return ``owner/name`` or raise ``ValueError`` for unsafe input."""
    value = value.strip().removesuffix(".git")
    if value.startswith("https://github.com/"):
        value = value.removeprefix("https://github.com/")
    elif value.startswith("http://github.com/"):
        value = value.removeprefix("http://github.com/")
    elif value.startswith("git@github.com:"):
        value = value.removeprefix("git@github.com:")
    if not REPOSITORY_PATTERN.fullmatch(value):
        raise ValueError(f"expected GitHub owner/name, got {value!r}")
    return value


def repository_from_remote(remote: str | None) -> str | None:
    """Extract ``owner/name`` from common HTTPS and SSH GitHub remotes."""
    if not remote:
        return None
    remote = remote.strip()
    if remote.startswith("git@github.com:"):
        candidate = remote.removeprefix("git@github.com:")
    else:
        parsed = urlparse(remote)
        if parsed.hostname != "github.com":
            return None
        candidate = parsed.path.lstrip("/")
    try:
        return normalize_repository(candidate)
    except ValueError:
        return None


def _configured_repository(root: Path) -> tuple[str, str]:
    env_value = os.environ.get("SIMPLICIO_GITHUB_REPOSITORY")
    if env_value:
        return normalize_repository(env_value), "environment"

    contract = root / ".simplicio" / "repository.json"
    if contract.is_file():
        data = json.loads(contract.read_text(encoding="utf-8"))
        value = data.get("github_repository")
        if value is None:
            value = data.get("github", {}).get("repository")
        if value:
            return normalize_repository(value), "workspace-contract"

    return CANONICAL_REPOSITORY, "product-default"


def _origin(root: Path) -> str | None:
    try:
        return subprocess.run(
            ["git", "-C", str(root), "remote", "get-url", "origin"],
            check=True,
            capture_output=True,
            text=True,
        ).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        return None


@dataclass(frozen=True)
class Resolution:
    status: str
    repository: str
    source: str
    remote: str | None
    remote_repository: str | None
    reason: str

    @property
    def ok(self) -> bool:
        return self.status in {"ready", "explicit_override"}

    def to_dict(self) -> dict[str, object]:
        return {
            "schema": "simplicio.github-repository/v1",
            "status": self.status,
            "ok": self.ok,
            "repository": self.repository,
            "source": self.source,
            "remote": self.remote,
            "remote_repository": self.remote_repository,
            "reason": self.reason,
        }


def resolve(root: Path, explicit: str | None = None, remote: str | None = None) -> Resolution:
    """Resolve and verify the repository authority for ``root``."""
    if explicit:
        repository = normalize_repository(explicit)
        source = "cli"
    else:
        repository, source = _configured_repository(root)

    remote = _origin(root) if remote is None else remote
    remote_repository = repository_from_remote(remote)
    if remote is None:
        return Resolution(
            "remote_missing", repository, source, remote, remote_repository,
            "origin is missing; refuse GitHub mutation until the workspace remote is verified",
        )
    if remote_repository is None:
        return Resolution(
            "remote_unrecognized", repository, source, remote, remote_repository,
            "origin is not a GitHub repository URL; refuse implicit issue/PR operations",
        )
    if remote_repository != repository:
        if explicit:
            return Resolution(
                "explicit_override", repository, source, remote, remote_repository,
                "CLI --repo explicitly selected a repository different from origin",
            )
        return Resolution(
            "remote_mismatch", repository, source, remote, remote_repository,
            "workspace contract and origin disagree; pass --repo only after reviewing the target",
        )
    return Resolution("ready", repository, source, remote, remote_repository, "contract and origin agree")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Fail-closed GitHub repository preflight")
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--repo", help="Explicit owner/name override, equivalent to gh --repo")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    try:
        result = resolve(args.root.resolve(), explicit=args.repo)
    except (OSError, ValueError, json.JSONDecodeError) as exc:
        print(f"error: cannot resolve GitHub repository: {exc}", file=sys.stderr)
        return 3
    if args.json:
        print(json.dumps(result.to_dict(), indent=2, sort_keys=True))
    else:
        print(f"github repository preflight: {result.status}")
        print(f"  repository: {result.repository}")
        print(f"  remote:     {result.remote or '(missing)'}")
        print(f"  reason:     {result.reason}")
    return 0 if result.ok else 2


if __name__ == "__main__":
    raise SystemExit(main())
