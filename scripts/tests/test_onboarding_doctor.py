import json
import os
from pathlib import Path
import stat
import socket
import sys

import pytest

sys.path.insert(0, str(Path(__file__).parents[1]))
import onboarding_doctor as subject


ROOT = Path(__file__).parents[2]
MANIFEST = ROOT / "config/onboarding-bundle-v1.json"


def test_manifest_is_complete_and_pinned():
    manifest = subject.load_manifest(MANIFEST)
    assert {x["name"] for x in manifest["components"]} == subject.COMPONENTS
    assert all(subject.VERSION.fullmatch(x["version"]) for x in manifest["components"])


def test_protocol_only_never_grants_authority(monkeypatch):
    monkeypatch.setattr(subject.shutil, "which", lambda _: None)
    report = subject.doctor(MANIFEST, ROOT, "protocol_only")
    assert report["status"] == "ready"
    assert report["effect_authority"] is False
    assert report["productive_ready"] is False


def test_productive_fails_closed_when_components_are_missing(monkeypatch):
    monkeypatch.setattr(subject.shutil, "which", lambda _: None)
    report = subject.doctor(MANIFEST, ROOT, "productive")
    assert report["status"] == "blocked"
    assert "AgentHost" in report["blocker"]


def test_probe_uses_argv_and_redacts_error(tmp_path):
    probe = tmp_path / "safe-probe"
    probe.write_text("#!/bin/sh\necho 'token=supersecret error' >&2\nexit 2\n")
    probe.chmod(probe.stat().st_mode | stat.S_IXUSR)
    version, error = subject._version(str(probe))
    assert version is None
    assert error == "version probe exited 2"
    assert subject.redact("api_key=hunter2") == "api_key=[REDACTED]"


def test_socket_rejects_regular_file_and_world_writable_socket(tmp_path):
    regular = tmp_path / "transport"
    regular.write_text("not a socket")
    assert subject._socket_check(str(regular))["health"] == "blocked"
    assert subject._socket_check(None)["path"] is None


def test_socket_accepts_private_unix_socket(tmp_path):
    target = tmp_path / "agent.sock"
    server = socket.socket(socket.AF_UNIX)
    try:
        server.bind(str(target))
        os.chmod(target, 0o600)
        assert subject._socket_check(str(target))["health"] == "ready"
    finally:
        server.close()


def test_version_probe_parses_semver(tmp_path):
    probe = tmp_path / "probe"
    probe.write_text("#!/bin/sh\necho 'probe 1.2.3'\n")
    probe.chmod(0o700)
    assert subject._version(str(probe)) == ("1.2.3", None)
    version, error = subject._version(str(tmp_path / "missing"))
    assert version is None and error


def test_socket_reports_missing_path(tmp_path):
    assert subject._socket_check(str(tmp_path / "missing"))["health"] == "blocked"


def test_invalid_manifest_returns_safe_error(tmp_path, capsys):
    manifest = tmp_path / "bad.json"
    manifest.write_text(json.dumps({"schema": "bad", "token": "secret"}))
    assert subject.main(["--manifest", str(manifest)]) == 1
    output = json.loads(capsys.readouterr().out)
    assert output["effect_authority"] is False


def test_cli_report_contains_required_component_fields(capsys):
    assert subject.main(["--manifest", str(MANIFEST), "--root", str(ROOT)]) == 0
    report = json.loads(capsys.readouterr().out)
    assert report["metrics"]["preflight_ns"] >= 0
    for component in report["components"]:
        assert {"expected_version", "capabilities", "origin", "health", "blocker"} <= component.keys()
