#!/usr/bin/env python3
"""Bounded offline clean-install, upgrade, and rollback evidence harness."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile


class LifecycleRejected(RuntimeError):
    """The lifecycle proof cannot safely continue."""


def _script_header(path: Path) -> bytes:
    with path.open("rb") as source:
        return source.readline(256)


def _windows_runnable(path: Path) -> bool:
    """Accept native PE files and explicitly interpreted test fixtures."""
    header = _script_header(path)
    return header.startswith(b"MZ") or header.startswith(b"#!")


def _shell_interpreter() -> str | None:
    candidates = [
        shutil.which("sh"),
        os.environ.get("ProgramFiles", "") + r"\Git\usr\bin\sh.exe",
        os.environ.get("ProgramFiles", "") + r"\Git\bin\sh.exe",
    ]
    return next(
        (candidate for candidate in candidates if candidate and Path(candidate).is_file()),
        None,
    )


def _command_for_artifact(path: Path) -> list[str]:
    """Run native artifacts directly and shebang fixtures through their interpreter."""
    if os.name != "nt":
        return [str(path)]
    header = _script_header(path)
    if header.startswith(b"MZ"):
        return [str(path)]
    if header.startswith(b"#!"):
        shebang = header[2:].decode("utf-8", errors="replace").lower()
        if "sh" in shebang:
            interpreter = _shell_interpreter()
            if interpreter:
                return [interpreter, str(path)]
        if "python" in shebang:
            return [sys.executable, str(path)]
    raise LifecycleRejected("artifact_interpreter_unavailable")


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(chunk)
    return "sha256:" + digest.hexdigest()


def _artifact(path: Path, label: str) -> dict[str, str]:
    resolved = path.resolve()
    if not resolved.is_file():
        raise LifecycleRejected(f"{label}_artifact_missing")
    if os.name == "nt" and not _windows_runnable(resolved):
        raise LifecycleRejected(f"{label}_artifact_not_executable")
    if not os.access(resolved, os.X_OK):
        raise LifecycleRejected(f"{label}_artifact_not_executable")
    return {"path": str(resolved), "digest": _sha256(resolved)}


def discover(old: Path | None, new: Path | None) -> tuple[Path, Path, str]:
    """Prefer explicit artifacts, then actual installed/environment discovery."""
    if old is not None or new is not None:
        if old is None or new is None:
            raise LifecycleRejected("both_old_and_new_artifacts_are_required")
        return old, new, "explicit"
    installed = os.environ.get("SIMPLICIO_CODE_INSTALLED_BIN") or shutil.which(
        "simplicio-code"
    )
    upgrade = os.environ.get("SIMPLICIO_CODE_UPGRADE_BIN")
    if not installed:
        raise LifecycleRejected("installed_artifact_missing")
    if not upgrade:
        raise LifecycleRejected("upgrade_artifact_missing")
    return Path(installed), Path(upgrade), "installed_discovery"


def _observe(binary: Path) -> dict[str, object]:
    observations: dict[str, object] = {}
    for name, arguments in (("version", ["--version"]), ("probe", ["probe"])):
        completed = subprocess.run(
            [*_command_for_artifact(binary), *arguments],
            capture_output=True,
            text=True,
            timeout=10,
            check=False,
            env={"PATH": os.environ.get("PATH", "")},
            stdin=subprocess.DEVNULL,
        )
        if completed.returncode != 0:
            raise LifecycleRejected(f"{name}_failed_exit_{completed.returncode}")
        observations[name] = completed.stdout.rstrip("\n")
    return observations


def _install(source: Path, target: Path) -> None:
    target.mkdir(mode=0o700)
    shutil.copyfile(source, target / "simplicio-code")
    (target / "simplicio-code").chmod(0o500)
    if _sha256(source) != _sha256(target / "simplicio-code"):
        raise LifecycleRejected("installed_digest_mismatch")


def _cleanup_prefix(prefix: Path) -> None:
    """Remove failed staging even when Windows marks copied artifacts read-only."""
    def onerror(function, path, _exc_info):
        try:
            Path(path).chmod(0o700)
            function(path)
        except OSError:
            pass

    try:
        shutil.rmtree(prefix, onerror=onerror)
    except OSError:
        pass


def run(prefix: Path, old_path: Path, new_path: Path, source: str) -> dict[str, object]:
    """Run only inside a caller-provided, initially absent installation prefix."""
    if prefix.exists():
        raise LifecycleRejected("clean_prefix_already_exists")
    old, new = _artifact(old_path, "old"), _artifact(new_path, "new")
    if old["digest"] == new["digest"]:
        raise LifecycleRejected("upgrade_artifact_matches_installed_artifact")
    prefix.mkdir(mode=0o700, parents=True)
    active, previous = prefix / "active", prefix / "previous"
    try:
        _install(Path(old["path"]), active)
        clean = _observe(active / "simplicio-code")
        if _sha256(active / "simplicio-code") != old["digest"]:
            raise LifecycleRejected("clean_install_digest_mismatch")

        staged = prefix / ".upgrade"
        _install(Path(new["path"]), staged)
        upgrade_canary = _observe(staged / "simplicio-code")
        os.replace(active, previous)
        try:
            os.replace(staged, active)
        except BaseException:
            os.replace(previous, active)
            raise
        upgraded = _observe(active / "simplicio-code")
        if upgraded != upgrade_canary or _sha256(active / "simplicio-code") != new["digest"]:
            raise LifecycleRejected("upgrade_observation_mismatch")

        swap = prefix / ".rollback"
        os.replace(active, swap)
        try:
            os.replace(previous, active)
            os.replace(swap, previous)
        except BaseException:
            if swap.exists() and not active.exists():
                os.replace(swap, active)
            raise
        rolled_back = _observe(active / "simplicio-code")
        if rolled_back != clean or _sha256(active / "simplicio-code") != old["digest"]:
            raise LifecycleRejected("rollback_observation_mismatch")
        if _sha256(previous / "simplicio-code") != new["digest"]:
            raise LifecycleRejected("rollback_previous_digest_mismatch")
        return {
            "schema": "simplicio.code-install-lifecycle-e2e/v1",
            "status": "passed",
            "proof_kind": "offline_fixture_non_release_proof" if source == "fixture" else "installed_artifacts",
            "artifact_source": source,
            "artifacts": {"old": {"digest": old["digest"]}, "new": {"digest": new["digest"]}},
            "clean_install": {"observed": clean, "digest": old["digest"]},
            "upgrade": {"observed": upgraded, "digest": new["digest"], "inactive_canary_passed": True},
            "rollback": {"observed": rolled_back, "digest": old["digest"], "previous_digest": new["digest"]},
            "unobserved": {
                "windows": {"value": None, "reason": "not observed by this platform-neutral local run"},
                "macos": {"value": None, "reason": "not observed by this platform-neutral local run"},
                "production_release": {"value": None, "reason": "offline artifacts do not prove publisher or production release behavior"},
            },
            "issue_closure_claimed": False,
        }
    except BaseException:
        _cleanup_prefix(prefix)
        raise


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--prefix", type=Path)
    parser.add_argument("--old", type=Path)
    parser.add_argument("--new", type=Path)
    parser.add_argument("--fixture", action="store_true")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    try:
        if args.fixture:
            if args.old or args.new:
                raise LifecycleRejected("fixture_conflicts_with_explicit_artifacts")
            old, new, source = (root / "scripts/fixtures/lifecycle/v1/simplicio-code", root / "scripts/fixtures/lifecycle/v2/simplicio-code", "fixture")
        else:
            old, new, source = discover(args.old, args.new)
        if args.prefix:
            receipt = run(args.prefix.resolve(), old, new, source)
        else:
            with tempfile.TemporaryDirectory(prefix="simplicio-lifecycle-parent-") as parent:
                receipt = run(Path(parent) / "install", old, new, source)
    except (LifecycleRejected, OSError, subprocess.SubprocessError) as error:
        print(json.dumps({"schema": "simplicio.code-install-lifecycle-e2e/v1", "status": "blocked", "reason": str(error)}, sort_keys=True))
        return 2
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
