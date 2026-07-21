from scripts.ci_diagnostics import collect


def test_diagnostics_do_not_dump_environment(tmp_path, monkeypatch):
    monkeypatch.setenv("GITHUB_ACTIONS", "true")
    monkeypatch.setenv("OPENAI_API_KEY", "secret-shaped-value")
    (tmp_path / ".github" / "workflows").mkdir(parents=True)
    (tmp_path / ".github" / "workflows" / "ci.yml").write_text("name: CI\n", encoding="utf-8")
    result = collect(tmp_path)
    assert result["schema"] == "simplicio.ci-diagnostics/v1"
    assert "OPENAI_API_KEY" not in str(result)
    assert result["github"] == {"GITHUB_ACTIONS": "true"}


def test_diagnostics_records_missing_commands_without_raising(tmp_path):
    result = collect(tmp_path)
    assert result["commands"]
    assert all("command" in item for item in result["commands"])
