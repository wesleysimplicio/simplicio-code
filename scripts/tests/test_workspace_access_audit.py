import json
from pathlib import Path

import pytest

from scripts.audit_workspace_access import SCHEMA, audit, main


def _manifest(path: Path, rules: list[dict], baseline: list[dict] | None = None) -> Path:
    spec = {"schema": SCHEMA, "scopes": ["src"], "rules": rules}
    if baseline is not None:
        spec["baseline"] = baseline
    path.write_text(json.dumps(spec), encoding="utf-8")
    return path


def test_unclassified_direct_access_fails_closed(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.rs").write_text('let _ = Command::new("git");\n', encoding="utf-8")
    result = audit(tmp_path, _manifest(tmp_path / "manifest.json", []))
    assert result["status"] == "failed"
    assert result["summary"]["unclassified"] == 1


def test_allowlisted_access_is_passed(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.rs").write_text('let _ = Command::new("git");\n', encoding="utf-8")
    result = audit(
        tmp_path,
        _manifest(
            tmp_path / "manifest.json",
            [{"path": "src/**", "kind": "process", "classification": "bootstrap-allowlisted", "owner": "test", "rationale": "metadata"}],
        ),
    )
    assert result["status"] == "passed"
    assert result["summary"] == {"total": 1, "violations": 0, "unclassified": 0, "baseline_errors": 0}


def test_explicit_violation_is_reported(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.rs").write_text("let _ = tokio::fs::read(path).await;\n", encoding="utf-8")
    result = audit(
        tmp_path,
        _manifest(
            tmp_path / "manifest.json",
            [{"path": "src/**", "kind": "filesystem", "classification": "violation", "owner": "migration", "rationale": "pending"}],
        ),
    )
    assert result["status"] == "failed"
    assert result["violations"][0]["kind"] == "filesystem"


def test_baseline_rejects_new_occurrence_hidden_by_broad_rule(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.rs").write_text(
        'let _ = Command::new("git");\nlet _ = Command::new("jj");\n', encoding="utf-8"
    )
    rule = {"path": "src/**", "kind": "process", "classification": "bootstrap-allowlisted", "owner": "bootstrap", "rationale": "metadata"}
    baseline = [{"path": "src/main.rs", "kind": "process", "classification": "bootstrap-allowlisted", "max_count": 1}]
    result = audit(tmp_path, _manifest(tmp_path / "manifest.json", [rule], baseline))
    assert result["status"] == "failed"
    assert result["baseline_errors"] == [{
        "path": "src/main.rs", "kind": "process", "classification": "bootstrap-allowlisted", "observed": 2, "max_count": 1
    }]


def test_baseline_allows_removing_an_occurrence(tmp_path):
    (tmp_path / "src").mkdir()
    (tmp_path / "src" / "main.rs").write_text('let _ = Command::new("git");\n', encoding="utf-8")
    rule = {"path": "src/**", "kind": "process", "classification": "bootstrap-allowlisted", "owner": "bootstrap", "rationale": "metadata"}
    baseline = [{"path": "src/main.rs", "kind": "process", "classification": "bootstrap-allowlisted", "max_count": 2}]
    result = audit(tmp_path, _manifest(tmp_path / "manifest.json", [rule], baseline))
    assert result["status"] == "passed"


@pytest.mark.parametrize(
    ("override", "message"),
    [
        ({"schema": "wrong"}, "unsupported manifest schema"),
        ({"rules": {}}, "manifest rules must be a list"),
        ({"baseline": {}}, "manifest baseline must be a list"),
        ({"baseline": ["bad"]}, "baseline entries must be objects"),
        ({"baseline": [{}]}, "baseline entries require"),
        ({"baseline": [{"path": "x", "kind": "process", "classification": "generated", "max_count": -1}]}, "non-negative max_count"),
    ],
)
def test_malformed_manifest_fails_closed(tmp_path, override, message):
    spec = {"schema": SCHEMA, "scopes": ["missing"], "rules": []} | override
    manifest = tmp_path / "manifest.json"
    manifest.write_text(json.dumps(spec), encoding="utf-8")
    with pytest.raises(ValueError, match=message):
        audit(tmp_path, manifest)


def test_main_reports_manifest_error_without_traceback(tmp_path, capsys):
    manifest = tmp_path / "bad.json"
    manifest.write_text("{", encoding="utf-8")
    assert main(["--root", str(tmp_path), "--manifest", "bad.json"]) == 1
    assert json.loads(capsys.readouterr().out)["status"] == "error"
