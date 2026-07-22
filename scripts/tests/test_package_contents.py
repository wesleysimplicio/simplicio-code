import pathlib
import subprocess
import sys


ROOT = pathlib.Path(__file__).parents[2]
SCRIPT = ROOT / "scripts" / "check_package_contents.py"


def run(root: pathlib.Path, inventory: pathlib.Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(
        [sys.executable, str(SCRIPT), str(root), "--inventory", str(inventory)],
        cwd=ROOT,
        text=True,
        capture_output=True,
        check=False,
    )


def test_package_scan_rejects_unreviewed_internal_artifact(tmp_path: pathlib.Path) -> None:
    (tmp_path / "session-state.json").write_text("{}", encoding="utf-8")
    inventory = tmp_path / "inventory.toml"
    inventory.write_text('schema = "v1"\n', encoding="utf-8")
    result = run(tmp_path, inventory)
    assert result.returncode == 1
    assert "session-state.json" in result.stdout


def test_package_scan_accepts_exact_external_manifest(tmp_path: pathlib.Path) -> None:
    (tmp_path / "manifest.json").write_text("{}", encoding="utf-8")
    inventory = tmp_path / "inventory.toml"
    inventory.write_text(
        'schema = "v1"\n[[package_output]]\npath = "manifest.json"\nowner = "release"\nreason = "external"\nexpires = "2099-12-31"\ncategory = "toolchain_mandated"\ntarget_format = "manifest"\nstatus = "exception"\nproducer = "release tooling"\nconsumer = "installer"\nlifecycle = "package output"\n',
        encoding="utf-8",
    )
    assert run(tmp_path, inventory).returncode == 0


def test_package_scan_rejects_unlisted_json_even_without_internal_name(tmp_path: pathlib.Path) -> None:
    (tmp_path / "new-contract.json").write_text("{}", encoding="utf-8")
    inventory = tmp_path / "inventory.toml"
    inventory.write_text('schema = "v1"\n', encoding="utf-8")
    result = run(tmp_path, inventory)
    assert result.returncode == 1
    assert "new-contract.json" in result.stdout


def test_package_scan_rejects_wildcard_exception(tmp_path: pathlib.Path) -> None:
    inventory = tmp_path / "inventory.toml"
    inventory.write_text(
        'schema = "v1"\n[[package_output]]\npath = "*.json"\nowner = "release"\nreason = "external"\nexpires = "2099-12-31"\ncategory = "toolchain_mandated"\ntarget_format = "manifest"\nstatus = "exception"\nproducer = "release tooling"\nconsumer = "installer"\nlifecycle = "package output"\n',
        encoding="utf-8",
    )
    result = run(tmp_path, inventory)
    assert result.returncode == 2
    assert "must be exact" in result.stderr


def test_package_scan_rejects_expired_exception(tmp_path: pathlib.Path) -> None:
    inventory = tmp_path / "inventory.toml"
    inventory.write_text(
        'schema = "v1"\n[[package_output]]\npath = "manifest.json"\nowner = "release"\nreason = "external"\nexpires = "2020-01-01"\ncategory = "toolchain_mandated"\ntarget_format = "manifest"\nstatus = "exception"\nproducer = "release tooling"\nconsumer = "installer"\nlifecycle = "package output"\n',
        encoding="utf-8",
    )
    result = run(tmp_path, inventory)
    assert result.returncode == 2
    assert "expired" in result.stderr


def test_package_scan_rejects_parent_and_absolute_exceptions(tmp_path: pathlib.Path) -> None:
    for path in ("../manifest.json", "/manifest.json"):
        inventory = tmp_path / "inventory.toml"
        inventory.write_text(
            f'schema = "v1"\n[[package_output]]\npath = "{path}"\nowner = "release"\nreason = "external"\nexpires = "2099-12-31"\ncategory = "toolchain_mandated"\ntarget_format = "manifest"\nstatus = "exception"\nproducer = "release tooling"\nconsumer = "installer"\nlifecycle = "package output"\n',
            encoding="utf-8",
        )
        result = run(tmp_path, inventory)
        assert result.returncode == 2
        assert "must be exact" in result.stderr
