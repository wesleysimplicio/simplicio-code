#!/usr/bin/env python3
"""Disk-space preflight check for local/CI workspace verification (issue #32).

Building and testing this workspace (`cargo build`/`cargo test`) can fail
with an opaque OS-level error ("No space left on device (os error 28)")
once the host disk fills up. That failure surfaces deep inside cargo's
fingerprinting/incremental-artifact machinery, well after a build has
started, so it wastes time and gives no actionable guidance.

This script is a *preflight* check, meant to run BEFORE a build/test
invocation (locally or in CI): it inspects free disk space against a
configurable minimum, measures the size of the caches that usually cause
the pressure (`target/`, `~/.cargo`, `.simplicio/`), and returns a
structured result — `disk_space_insufficient` when there isn't enough
room — instead of letting cargo fail with a raw OS error partway through
a build.

Explicitly out of scope, by design (see issue #32's acceptance criteria):
this script NEVER deletes anything. It only reports. Any cleanup policy
(what's safe to delete, retention windows, etc.) is a separate, deliberate
decision that must not be automated here without explicit review.

Usage:

    python3 scripts/check_disk_budget.py
    python3 scripts/check_disk_budget.py --json
    python3 scripts/check_disk_budget.py --min-free-gb 10 --path /some/workspace

Exit codes:
    0  sufficient free space
    2  insufficient free space (`disk_space_insufficient`)
    3  usage/argument error
"""
from __future__ import annotations

import argparse
import json
import os
import sys
from dataclasses import dataclass, field
from typing import Callable, Iterable

# Default minimum free space required before a build/test run is considered
# safe to attempt. Chosen conservatively: a `cargo test --workspace` build
# in this repo can transiently need several GiB of scratch space for
# incremental artifacts and fingerprints.
DEFAULT_MIN_FREE_BYTES = 5 * 1024 * 1024 * 1024  # 5 GiB

GIB = 1024 * 1024 * 1024


@dataclass
class CacheDirReport:
    """One measured cache directory: its label, path, and size on disk."""

    name: str
    path: str
    size_bytes: int
    exists: bool

    def to_dict(self) -> dict:
        return {
            "name": self.name,
            "path": self.path,
            "size_bytes": self.size_bytes,
            "exists": self.exists,
        }


@dataclass
class DiskBudgetResult:
    """Structured preflight result. `status` is the machine-readable field
    callers (perf_gate.py, CI, etc.) should branch on; everything else is
    supporting evidence and a human-actionable remediation string."""

    status: str  # "ok" | "disk_space_insufficient"
    free_bytes: int
    required_bytes: int
    deficit_bytes: int
    path: str
    caches: list = field(default_factory=list)
    remediation: str = ""

    @property
    def ok(self) -> bool:
        return self.status == "ok"

    def to_dict(self) -> dict:
        return {
            "schema": "simplicio.disk-budget/v1",
            "status": self.status,
            "ok": self.ok,
            "free_bytes": self.free_bytes,
            "required_bytes": self.required_bytes,
            "deficit_bytes": self.deficit_bytes,
            "path": self.path,
            "caches": [c.to_dict() if isinstance(c, CacheDirReport) else c for c in self.caches],
            "remediation": self.remediation,
        }


def _dir_size_bytes(path: str) -> int:
    """Sum of file sizes under `path`, recursively. Returns 0 for a
    missing/inaccessible path rather than raising, since a cache dir that
    doesn't exist yet (e.g. `target/` before the first build) is a normal,
    zero-size case, not an error."""
    if not os.path.isdir(path):
        return 0
    total = 0
    for dirpath, _dirnames, filenames in os.walk(path, onerror=lambda _e: None):
        for name in filenames:
            fp = os.path.join(dirpath, name)
            try:
                total += os.lstat(fp).st_size
            except OSError:
                # Race (file removed mid-walk) or permission error — best
                # effort, not fatal to the whole measurement.
                continue
    return total


def default_cache_dirs(workspace: str) -> list:
    """The cache directories issue #32 calls out by name, resolved relative
    to `workspace` (for `target/` and `.simplicio/`) or the user's home
    directory (for `~/.cargo`)."""
    home = os.path.expanduser("~")
    return [
        ("target/", os.path.join(workspace, "target")),
        ("~/.cargo", os.path.join(home, ".cargo")),
        (".simplicio/", os.path.join(workspace, ".simplicio")),
    ]


def measure_cache_dirs(
    cache_dirs: Iterable[tuple],
    size_fn: Callable[[str], int] = _dir_size_bytes,
) -> list:
    """Measure each (name, path) pair with `size_fn` (overridable for tests
    so we never have to actually fill/walk a real disk to test this)."""
    reports = []
    for name, path in cache_dirs:
        exists = os.path.isdir(path)
        size = size_fn(path) if exists else 0
        reports.append(CacheDirReport(name=name, path=path, size_bytes=size, exists=exists))
    return reports


def _format_gib(n: int) -> str:
    return f"{n / GIB:.2f} GiB"


def _remediation_text(caches: list, deficit_bytes: int) -> str:
    """Build a human-actionable remediation string. Intentionally never
    suggests an automatic/silent delete — only a command the operator runs
    themselves, and only for the largest offender, so review stays in the
    loop."""
    ranked = sorted(
        (c for c in caches if getattr(c, "exists", False)),
        key=lambda c: c.size_bytes,
        reverse=True,
    )
    if not ranked:
        return (
            f"Free at least {_format_gib(deficit_bytes)} more before retrying "
            "(no measured cache directory was found to suggest reclaiming from)."
        )
    biggest = ranked[0]
    return (
        f"Free at least {_format_gib(deficit_bytes)} more before retrying. "
        f"Largest measured cache is {biggest.name} ({_format_gib(biggest.size_bytes)}) at "
        f"{biggest.path}. Review before deleting, e.g.: "
        f"`cargo clean --manifest-path Cargo.toml` (for target/) or remove unused entries "
        f"under {biggest.path} manually. This tool does not delete anything for you."
    )


def check_disk_budget(
    free_bytes: int,
    required_bytes: int,
    path: str,
    caches: list,
) -> DiskBudgetResult:
    """Pure decision function: given already-measured facts, decide
    sufficient vs. insufficient. Kept separate from disk I/O so unit tests
    can exercise the sufficient/insufficient/boundary logic without ever
    touching a real filesystem."""
    if free_bytes >= required_bytes:
        return DiskBudgetResult(
            status="ok",
            free_bytes=free_bytes,
            required_bytes=required_bytes,
            deficit_bytes=0,
            path=path,
            caches=caches,
            remediation="",
        )
    deficit = required_bytes - free_bytes
    return DiskBudgetResult(
        status="disk_space_insufficient",
        free_bytes=free_bytes,
        required_bytes=required_bytes,
        deficit_bytes=deficit,
        path=path,
        caches=caches,
        remediation=_remediation_text(caches, deficit),
    )


def run_preflight(
    path: str,
    min_free_bytes: int = DEFAULT_MIN_FREE_BYTES,
    disk_usage_fn: Callable[[str], "os.statvfs_result"] = None,
    cache_dirs: Iterable[tuple] = None,
    size_fn: Callable[[str], int] = _dir_size_bytes,
) -> DiskBudgetResult:
    """End-to-end preflight: measure real disk free space and cache sizes,
    then apply `check_disk_budget`. `disk_usage_fn` and `size_fn` are
    injectable so tests can mock the filesystem instead of needing a real
    (or artificially filled) disk."""
    usage_fn = disk_usage_fn or (lambda p: __import__("shutil").disk_usage(p))
    usage = usage_fn(path)
    free_bytes = usage.free

    dirs = list(cache_dirs) if cache_dirs is not None else default_cache_dirs(path)
    caches = measure_cache_dirs(dirs, size_fn=size_fn)

    return check_disk_budget(free_bytes, min_free_bytes, path, caches)


def _print_human(result: DiskBudgetResult) -> None:
    print(f"disk budget check: {result.status}")
    print(f"  path:      {result.path}")
    print(f"  free:      {_format_gib(result.free_bytes)}")
    print(f"  required:  {_format_gib(result.required_bytes)}")
    if not result.ok:
        print(f"  deficit:   {_format_gib(result.deficit_bytes)}")
    print("  caches:")
    for c in result.caches:
        d = c.to_dict() if isinstance(c, CacheDirReport) else c
        marker = "" if d["exists"] else " (not present)"
        print(f"    - {d['name']:<12} {_format_gib(d['size_bytes'])}{marker}  [{d['path']}]")
    if result.remediation:
        print(f"  remediation: {result.remediation}")


def build_arg_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(
        description="Preflight disk-space check for build/test workspace verification.",
    )
    parser.add_argument(
        "--path",
        default=os.getcwd(),
        help="Workspace directory to check free space for (default: cwd).",
    )
    parser.add_argument(
        "--min-free-gb",
        type=float,
        default=DEFAULT_MIN_FREE_BYTES / GIB,
        help="Minimum required free space in GiB (default: %(default)s).",
    )
    parser.add_argument(
        "--json",
        action="store_true",
        help="Emit a structured simplicio.disk-budget/v1 JSON result instead of text.",
    )
    return parser


def main(argv=None) -> int:
    parser = build_arg_parser()
    args = parser.parse_args(argv)

    if args.min_free_gb <= 0:
        print("error: --min-free-gb must be positive", file=sys.stderr)
        return 3

    min_free_bytes = int(args.min_free_gb * GIB)
    result = run_preflight(args.path, min_free_bytes=min_free_bytes)

    if args.json:
        print(json.dumps(result.to_dict(), indent=2, sort_keys=True))
    else:
        _print_human(result)

    return 0 if result.ok else 2


if __name__ == "__main__":
    sys.exit(main())
