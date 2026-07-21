from scripts.ci_diagnostics import collect, redact


def test_diagnostics_use_only_allowlisted_environment(tmp_path, monkeypatch):
    monkeypatch.setenv("GITHUB_ACTIONS", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "must-not-appear")
    result = collect(tmp_path)
    assert result["github"] == {"GITHUB_ACTIONS": "true"}
    assert "OPENAI_API_KEY" not in json_text(result)


def test_redact_removes_secret_shaped_values():
    assert "super-secret" not in redact("token=super-secret")
    assert "[REDACTED]" in redact("token=super-secret")


def json_text(value):
    import json
    return json.dumps(value)
