from __future__ import annotations

import json
import sys
from pathlib import Path

import pytest

from scripts import recovery_evidence as MODULE


def _fake_runner(
    argv: list[str] | tuple[str, ...], _cwd: Path, _timeout: float
) -> MODULE.CommandResult:
    if argv[:2] == ["git", "rev-parse"]:
        return MODULE.CommandResult("PASS", 0, 1, "abc123\n")
    if argv[:2] == ["git", "status"]:
        return MODULE.CommandResult("PASS", 0, 1, "")
    versions = {
        "runtime-bin": "3.6.0",
        "mapper-bin": "0.26.15",
        "fast-bin": "2.0.24",
        "dev-bin": "0.18.6",
        "loop-bin": "3.38.35",
    }
    version = versions.get(Path(argv[0]).name, "0.0.0")
    payload: dict[str, object] = {"version": version}
    if Path(argv[0]).name == "runtime-bin":
        payload = {"version": "0.2.0", "runtime": {"version": version}}
    return MODULE.CommandResult("PASS", 0, 1, json.dumps(payload))


def _tool_paths() -> dict[str, str]:
    return {
        "runtime": "runtime-bin",
        "mapper": "mapper-bin",
        "fast": "fast-bin",
        "dev-cli": "dev-bin",
        "loop": "loop-bin",
    }


def test_collect_records_toolchain_and_fails_closed_when_validation_is_skipped(
    tmp_path: Path,
) -> None:
    report = MODULE.collect(
        tmp_path,
        tmp_path / "out",
        run_validation=False,
        runner=_fake_runner,
        tool_paths=_tool_paths(),
    )
    assert report["schema"] == "simplicio.code-recovery-evidence/v1"
    assert report["status"] == "UNVERIFIED"
    assert [tool["version"] for tool in report["tools"]] == [
        "3.6.0",
        "0.26.15",
        "2.0.24",
        "0.18.6",
        "3.38.35",
    ]
    assert report["local_validation"]["status"] == "UNVERIFIED"


def test_missing_tool_is_never_a_pass(tmp_path: Path) -> None:
    result = MODULE.probe_tool(
        "mapper", None, ("--version",), tmp_path, 1, _fake_runner
    )
    assert result["status"] == "UNAVAILABLE"
    assert result["exit_code"] == 127


def test_untrustworthy_version_is_a_failure(tmp_path: Path) -> None:
    def runner(_argv: list[str], _cwd: Path, _timeout: float) -> MODULE.CommandResult:
        return MODULE.CommandResult("PASS", 0, 1, "development build")

    result = MODULE.probe_tool(
        "mapper", "mapper-bin", ("--version",), tmp_path, 1, runner
    )
    assert result["status"] == "FAIL"
    assert "trustworthy semver" in result["output_tail"]


def test_secret_shaped_output_is_redacted(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIMPLICIO_TOKEN", "environment-secret")
    value = MODULE._redact(
        "token=shape-secret password:quoted-secret api_key = environment-secret"
    )
    assert "shape-secret" not in value
    assert "quoted-secret" not in value
    assert "environment-secret" not in value
    assert value.count("[REDACTED]") == 3


def test_bundle_digest_detects_tampering(tmp_path: Path) -> None:
    report = MODULE.collect(
        tmp_path,
        tmp_path / "out",
        run_validation=False,
        runner=_fake_runner,
        tool_paths=_tool_paths(),
    )
    paths = MODULE.write_bundle(report, tmp_path / "out")
    evidence = Path(paths["json"])
    assert MODULE.verify_report(evidence)["evidence_sha256"] == report["evidence_sha256"]
    payload = json.loads(evidence.read_text(encoding="utf-8"))
    payload["status"] = "PASS"
    evidence.write_text(json.dumps(payload), encoding="utf-8")
    with pytest.raises(ValueError, match="digest mismatch"):
        MODULE.verify_report(evidence)


def test_timeout_is_not_pass(tmp_path: Path) -> None:
    result = MODULE.run_command(
        [sys.executable, "-c", "import time; time.sleep(1)"],
        tmp_path,
        0.05,
    )
    assert result.status == "TIMEOUT"
    assert result.exit_code == 124
