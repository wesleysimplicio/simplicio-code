import json
from pathlib import Path

from scripts.audit_workspace_access import SCHEMA, audit


def _manifest(path: Path, rules: list[dict]) -> Path:
    path.write_text(json.dumps({"schema": SCHEMA, "scopes": ["src"], "rules": rules}), encoding="utf-8")
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
    assert result["summary"] == {"total": 1, "violations": 0, "unclassified": 0}


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
