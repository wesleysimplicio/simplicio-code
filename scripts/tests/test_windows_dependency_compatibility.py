from pathlib import Path
import tomllib


REPO = Path(__file__).resolve().parents[2]
SHELL_MANIFEST = REPO / "crates" / "codegen" / "xai-grok-shell" / "Cargo.toml"
LOCKFILE = REPO / "Cargo.lock"


def test_process_wrap_stays_compatible_with_workspace_windows_crate():
    manifest = tomllib.loads(SHELL_MANIFEST.read_text(encoding="utf-8"))
    dependency = manifest["dependencies"]["process-wrap"]
    assert dependency["version"] == "=9.0.0"

    lock = tomllib.loads(LOCKFILE.read_text(encoding="utf-8"))
    process_wrap = next(package for package in lock["package"] if package["name"] == "process-wrap")
    assert process_wrap["version"] == "9.0.0"
    assert "windows 0.61.3" in process_wrap["dependencies"]