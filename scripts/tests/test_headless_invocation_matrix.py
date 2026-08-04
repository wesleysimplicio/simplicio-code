import pytest

from scripts import headless_invocation_matrix as matrix


def test_matrix_contains_all_permission_prompt_and_tty_cells():
    cases = matrix.build_cases()
    assert len(cases) == 28
    assert len({case.name for case in cases}) == 28
    assert sum(case.tty for case in cases) == 14
    assert sum(not case.tty for case in cases) == 14


def test_windows_fatal_codes_are_classified_as_crashes():
    outcome, reason = matrix._classify_returncode(0xC0000005)
    assert outcome == "crash"
    assert reason == "process_crash_c0000005"


def test_timeout_argument_is_rejected_before_execution():
    with pytest.raises(SystemExit):
        matrix.main(["--timeout-seconds", "0"])
