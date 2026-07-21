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
        'schema = "v1"\n[[boundary]]\npath = "manifest.json"\nowner = "release"\nreason = "external"\nexpires = "2099-12-31"\ncategory = "toolchain_mandated"\ntarget_format = "manifest"\nstatus = "exception"\n',
        encoding="utf-8",
    )
    assert run(tmp_path, inventory).returncode == 0
