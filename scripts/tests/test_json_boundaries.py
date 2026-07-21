import importlib.util
import pathlib


ROOT = pathlib.Path(__file__).parents[2]
SPEC = importlib.util.spec_from_file_location("json_boundaries", ROOT / "scripts" / "check_json_boundaries.py")
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def test_scans_extensionless_embedded_and_artifact_boundaries(tmp_path: pathlib.Path) -> None:
    (tmp_path / "renamed.data").write_text('JSON.parse("{}")\n', encoding="utf-8")
    (tmp_path / "state.json").write_text("{}", encoding="utf-8")
    findings = MODULE.findings(tmp_path)
    assert ("state.json", 1, "artifact:.json") in findings
    assert any(path == "renamed.data" and token == "JSON.parse" for path, _, token in findings)


def test_scans_generated_output_only_when_requested(tmp_path: pathlib.Path) -> None:
    generated = tmp_path / "target" / "package"
    generated.mkdir(parents=True)
    (generated / "session.json").write_text("{}", encoding="utf-8")
    assert not MODULE.findings(tmp_path)
    assert MODULE.findings(tmp_path, include_generated=True) == [("target/package/session.json", 1, "artifact:.json")]
