from __future__ import annotations

from pathlib import Path
import json
import sys
from urllib.error import HTTPError
from urllib.request import urlopen

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from preview_server import browser_result, serve_target  # noqa: E402


def test_file_target_serves_only_the_staged_report_and_stops() -> None:
    import tempfile

    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        report = root / "report.html"
        secret = root / "workspace-secret.txt"
        report.write_text("<h1>ok</h1>", encoding="utf-8")
        secret.write_text("must not be served", encoding="utf-8")
        with serve_target(report.as_uri()) as preview:
            assert urlopen(preview.url, timeout=2).read() == b"<h1>ok</h1>"
            try:
                urlopen(f"{preview.url.rsplit('/', 1)[0]}/workspace-secret.txt", timeout=2)
            except HTTPError as error:
                assert error.code == 404
            else:  # pragma: no cover - fail closed if staging leaks the source directory
                raise AssertionError("preview exposed a file outside the staged report")
            assert preview.staged.exists()
        assert not preview.staged.exists()


def test_browser_unavailable_is_not_executed() -> None:
    result = browser_result(False)
    assert result == {"status": "NOT_EXECUTED", "reason": "browser_unavailable"}
    assert json.dumps(result) == '{"status": "NOT_EXECUTED", "reason": "browser_unavailable"}'


def test_existing_browser_evidence_remains_external() -> None:
    assert browser_result(True) == {
        "status": "UNVERIFIED",
        "reason": "external_browser_evidence_required",
    }
