import importlib.util
import pathlib
import tomllib


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


def test_ignores_non_code_logs_from_boundary_inventory(tmp_path: pathlib.Path) -> None:
    (tmp_path / "error.log").write_text("json.dumps({})\n", encoding="utf-8")
    assert MODULE.findings(tmp_path) == []


def test_scans_generated_output_only_when_requested(tmp_path: pathlib.Path) -> None:
    generated = tmp_path / "target" / "package"
    generated.mkdir(parents=True)
    (generated / "session.json").write_text("{}", encoding="utf-8")
    assert not MODULE.findings(tmp_path)
    assert MODULE.findings(tmp_path, include_generated=True) == [("target/package/session.json", 1, "artifact:.json")]


def test_scans_generated_simplicio_artifacts_only_when_requested(tmp_path: pathlib.Path) -> None:
    generated = tmp_path / ".simplicio" / "history" / "run"
    generated.mkdir(parents=True)
    (generated / "manifest.json").write_text("{}", encoding="utf-8")
    assert not MODULE.findings(tmp_path)
    assert MODULE.findings(tmp_path, include_generated=True) == [
        (".simplicio/history/run/manifest.json", 1, "artifact:.json")
    ]


def test_scope_limits_findings_to_exact_paths(tmp_path: pathlib.Path) -> None:
    (tmp_path / "owned.py").write_text('import json\n', encoding="utf-8")
    (tmp_path / "other.py").write_text('import json\n', encoding="utf-8")
    scope = tmp_path / "scope.txt"
    scope.write_text("owned.py\n", encoding="utf-8")
    assert MODULE.load_scope(scope) == {"owned.py"}
    assert [item[0] for item in MODULE.findings(tmp_path) if item[0] in MODULE.load_scope(scope)] == ["owned.py"]


def test_scope_rejects_globs_and_parent_paths(tmp_path: pathlib.Path) -> None:
    scope = tmp_path / "scope.txt"
    scope.write_text("src/*.py\n", encoding="utf-8")
    try:
        MODULE.load_scope(scope)
    except ValueError as error:
        assert "exact" in str(error)
    else:
        raise AssertionError("glob scope must be rejected")


def test_every_repository_json_finding_has_an_exact_inventory_owner() -> None:
    """Keep the reviewed baseline complete as the repository evolves.

    Migration-pending entries remain visible to strict migration lanes, but a
    new JSON occurrence may not bypass classification merely because the broad
    repository audit runs in baseline mode.
    """
    inventory = MODULE.load_inventory(ROOT / "config" / "json-boundaries.toml")
    unclassified = sorted({path for path, _, _ in MODULE.findings(ROOT) if path not in inventory})
    assert unclassified == []


def test_inventory_requires_explicit_expiry_and_protects_internal_statuses() -> None:
    raw = tomllib.loads((ROOT / "config" / "json-boundaries.toml").read_text(encoding="utf-8"))
    groups = raw.get("boundary", []) + raw.get("audit", [])
    assert groups
    assert all(group.get("expires") for group in groups)

    inventory = MODULE.load_inventory(ROOT / "config" / "json-boundaries.toml")
    internal_categories = {
        "internal-evidence",
        "internal_cache",
        "internal_fixture_or_evidence",
        "internal_session_state",
        "internal_state_migration",
    }
    for path, entry in inventory.items():
        if entry["category"] in internal_categories:
            assert entry["status"] != "exception", path


def test_pending_findings_are_explicit_internal_migrations() -> None:
    inventory = MODULE.load_inventory(ROOT / "config" / "json-boundaries.toml")
    pending_paths = {
        path for path, _, _ in MODULE.findings(ROOT) if inventory[path]["status"] == "migration_pending"
    }
    assert pending_paths
    assert all(inventory[path]["category"].startswith("internal") for path in pending_paths)
    assert inventory["scripts/audit_workspace_access.py"]["status"] == "migration_pending"


def test_scope_validation_rejects_missing_or_directory_entries(tmp_path: pathlib.Path) -> None:
    (tmp_path / "directory").mkdir()
    for entry in ({"missing.rs"}, {"directory"}):
        try:
            MODULE.validate_scope(tmp_path, entry)
        except ValueError as error:
            assert "do not exist or are not files" in str(error)
        else:
            raise AssertionError("a stale strict scope must fail closed")


def test_inventory_rejects_noncanonical_paths_and_unknown_status(tmp_path: pathlib.Path) -> None:
    common = (
        'owner="quality"\nreason="reviewed"\nexpires="2099-12-31"\n'
        'category="test"\ntarget_format="HBI"\nproducer="test"\nconsumer="test"\n'
        'lifecycle="test"\n'
    )
    for path, status in (("../escape.rs", "exception"), ("/absolute.rs", "exception"), ("ok.rs", "ignored")):
        inventory = tmp_path / "inventory.toml"
        inventory.write_text(
            f'[[boundary]]\npath="{path}"\nstatus="{status}"\n{common}', encoding="utf-8"
        )
        try:
            MODULE.load_inventory(inventory)
        except ValueError:
            pass
        else:
            raise AssertionError("unsafe inventory entry must be rejected")
