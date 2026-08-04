import unittest
from contextlib import redirect_stderr
from io import StringIO

from scripts.preview_report import main, validate_url


class PreviewReportTests(unittest.TestCase):
    def test_file_url_fails_fast(self) -> None:
        with self.assertRaisesRegex(ValueError, "file:// is not supported"):
            validate_url("file:///tmp/report.html")

    def test_http_url_is_accepted(self) -> None:
        validate_url("http://127.0.0.1:8000/")

    def test_cli_reports_file_url_without_traceback(self) -> None:
        stderr = StringIO()
        with redirect_stderr(stderr), self.assertRaises(SystemExit) as raised:
            main([".", "--url", "file:///tmp/report.html"])
        self.assertEqual(raised.exception.code, 2)
        self.assertIn("file:// is not supported", stderr.getvalue())
        self.assertNotIn("Traceback", stderr.getvalue())


if __name__ == "__main__":
    unittest.main()
