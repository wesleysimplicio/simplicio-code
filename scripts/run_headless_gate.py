#!/usr/bin/env python3
"""Build the real Code binary and run the offline headless matrix gate."""
from __future__ import annotations

import argparse
import subprocess
import sys
from pathlib import Path


def run(command: list[str], root: Path, timeout: int) -> int:
    completed = subprocess.run(
        command,
        cwd=root,
        text=True,
        timeout=timeout,
        check=False,
    )
    return completed.returncode


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--root", type=Path, default=Path.cwd())
    parser.add_argument("--timeout-seconds", type=int, default=30)
    args = parser.parse_args(argv)
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")

    root = args.root.resolve()
    binary = root / "target" / "debug" / "simplicio-code"
    if sys.platform == "win32":
        binary = binary.with_suffix(".exe")

    build = [
        "cargo",
        "build",
        "-p",
        "xai-grok-pager-bin",
        "--bin",
        "simplicio-code",
    ]
    if run(build, root, timeout=600) != 0:
        return 1

    matrix = [
        sys.executable,
        "scripts/headless_invocation_matrix.py",
        "--binary",
        str(binary),
        "--mock-provider",
        "--timeout-seconds",
        str(args.timeout_seconds),
        "--json",
    ]
    return run(matrix, root, timeout=max(60, args.timeout_seconds * 10))


if __name__ == "__main__":
    raise SystemExit(main())
