import json
from pathlib import Path

import pytest

from scripts import host_bias_experiment as experiment


def record(record_id: str, host: str = "claude", variant: str = "bias", adoption: str = "adopted"):
    return {
        "schema": experiment.RECORD_SCHEMA,
        "experiment_id": "host-bias-v1",
        "record_id": record_id,
        "host": host,
        "host_version": "fixture",
        "variant": variant,
        "text_revision": "uwp-v3.1" if variant == "bias" else "control-v1",
        "consent": True,
        "adoption": adoption,
        "rollback": "not_requested",
        "recorded_at": "2026-09-02T00:00:00Z",
    }


def test_rejects_raw_prompt_response_and_identity_fields():
    item = record("r1")
    item["prompt"] = "secret"
    with pytest.raises(ValueError, match="prohibited"):
        experiment.validate_record(item)


def test_aggregate_is_redacted_deterministic_and_calculates_rate():
    result = experiment.aggregate(
        [
            record("r2", adoption="not_adopted"),
            record("r1"),
            record("r3", host="codex", variant="control", adoption="unknown"),
        ]
    )
    assert result["schema"] == experiment.AGGREGATE_SCHEMA
    assert result["redacted"] is True
    assert result["record_count"] == 3
    assert result["groups"][0]["host"] == "claude"
    assert result["groups"][0]["adoption_rate"] == 0.5
    assert "record_id" not in json.dumps(result)


def test_rollback_replaces_text_atomically_and_emits_digests(tmp_path: Path):
    active = tmp_path / "active.md"
    previous = tmp_path / "previous.md"
    output = tmp_path / "selected.md"
    active.write_text("new", encoding="utf-8")
    previous.write_text("old", encoding="utf-8")
    receipt = experiment.rollback_text(active, previous, output, "adoption below threshold")
    assert output.read_text(encoding="utf-8") == "old"
    assert receipt["schema"] == experiment.ROLLBACK_SCHEMA
    assert receipt["decision"] == "rolled_back"
    assert receipt["result_sha256"] == experiment._digest(previous)


def test_rollback_requires_previous_artifact(tmp_path: Path):
    with pytest.raises(ValueError, match="previous"):
        experiment.rollback_text(tmp_path / "active", tmp_path / "missing", tmp_path / "out", "reason")
