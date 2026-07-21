from pathlib import Path

import pytest

from scripts.check_coverage import coverage_percent


def test_coverage_percent_uses_lcov_line_totals(tmp_path: Path):
    report = tmp_path / "coverage.lcov"
    report.write_text("TN:\nSF:src/lib.rs\nLF:10\nLH:8\nend_of_record\n", encoding="utf-8")
    assert coverage_percent(report) == pytest.approx(80.0)


def test_coverage_percent_rejects_empty_report(tmp_path: Path):
    report = tmp_path / "coverage.lcov"
    report.write_text("TN:\nLF:0\nLH:0\n", encoding="utf-8")
    with pytest.raises(ValueError, match="no executable lines"):
        coverage_percent(report)
