from __future__ import annotations

import sys
import threading
from pathlib import Path

import pytest

from scripts import validate_local as MODULE


def test_redact_secret_shaped_output(monkeypatch: pytest.MonkeyPatch) -> None:
    monkeypatch.setenv("SIMPLICIO_TOKEN", "environment-secret")
    value = MODULE._redact(
        "token=shape-secret password:quoted-secret api_key = environment-secret"
    )
    assert "shape-secret" not in value
    assert "quoted-secret" not in value
    assert "environment-secret" not in value
    assert value.count("[REDACTED]") == 3


def test_timeout_is_a_failure_with_bounded_output(tmp_path: Path) -> None:
    gate = MODULE.Gate(
        "slow",
        [sys.executable, "-c", "import time; time.sleep(2)"],
    )
    gate.run(tmp_path, timeout=0.1)
    assert gate.status == "FAIL"
    assert gate.exit_code == 124
    assert "timeout" in gate.output


def test_cancelled_run_does_not_start_following_gates(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    marker = tmp_path / "started"
    second_command = [
        sys.executable,
        "-c",
        f"from pathlib import Path; Path({str(marker)!r}).write_text('started')",
    ]
    monkeypatch.setattr(
        MODULE,
        "gates",
        lambda _profile: [
            MODULE.Gate("first", [sys.executable, "-c", "pass"]),
            MODULE.Gate("second", second_command),
        ],
    )
    cancel_event = threading.Event()
    cancel_event.set()
    report = MODULE.run("fast", tmp_path, tmp_path / "out", 1, cancel_event)
    statuses = [gate["status"] for gate in report["gates"]]
    assert statuses == ["CANCELLED", "NOT_EXECUTED"]
    assert report["status"] == "CANCELLED"
    assert not marker.exists()


def test_failed_gate_marks_following_job_not_started(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    marker = tmp_path / "started"
    second_command = [
        sys.executable,
        "-c",
        f"from pathlib import Path; Path({str(marker)!r}).write_text('started')",
    ]
    monkeypatch.setattr(
        MODULE,
        "gates",
        lambda _profile: [
            MODULE.Gate("first", [sys.executable, "-c", "raise SystemExit(7)"]),
            MODULE.Gate("second", second_command),
        ],
    )
    report = MODULE.run("fast", tmp_path, tmp_path / "out", 1)
    statuses = [gate["status"] for gate in report["gates"]]
    assert statuses == ["FAIL", "NOT_EXECUTED"]
    assert "job was not started" in report["gates"][1]["output_tail"]
    assert not marker.exists()


def test_hbp_receipt_detects_tampered_gate_artifact(
    tmp_path: Path, monkeypatch: pytest.MonkeyPatch
) -> None:
    monkeypatch.setattr(
        MODULE,
        "gates",
        lambda _profile: [MODULE.Gate("ok", [sys.executable, "-c", "pass"])],
    )
    output = tmp_path / "out"
    report = MODULE.run("fast", tmp_path, output, 1)
    receipt = output / "validation-receipt.hbp"
    assert MODULE.verify_hbp(receipt)["receipt_sha256"] == report["receipt_sha256"]
    artifact = output / str(report["gates"][0]["artifact"])
    artifact.write_text(artifact.read_text(encoding="utf-8") + "tampered", encoding="utf-8")
    with pytest.raises(ValueError, match="artifact digest mismatch"):
        MODULE.verify_hbp(receipt)