import json
import os
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
if str(ROOT) not in sys.path:
    sys.path.insert(0, str(ROOT))

from scripts.ci_diagnostics import collect, redact


class CiDiagnosticsTests(unittest.TestCase):
    def test_diagnostics_use_only_allowlisted_environment(self):
        previous = {
            key: os.environ.get(key)
            for key in ("GITHUB_ACTIONS", "OPENAI_API_KEY")
        }
        os.environ["GITHUB_ACTIONS"] = "true"
        os.environ["OPENAI_API_KEY"] = "smoke-secret"
        try:
            result = collect(ROOT)
        finally:
            for key, value in previous.items():
                if value is None:
                    os.environ.pop(key, None)
                else:
                    os.environ[key] = value

        self.assertEqual(result["github"], {"GITHUB_ACTIONS": "true"})
        self.assertNotIn("smoke-secret", json.dumps(result))

    def test_redact_removes_secret_shaped_values(self):
        value = "token=super-secret password:also-secret api_key = third-secret"
        redacted = redact(value)
        self.assertNotIn("super-secret", redacted)
        self.assertNotIn("also-secret", redacted)
        self.assertNotIn("third-secret", redacted)
        self.assertEqual(redacted.count("[REDACTED]"), 3)

    def test_cli_smoke_writes_valid_bounded_artifact_without_credentials(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / "ci-diagnostics.json"
            env = os.environ.copy()
            env["OPENAI_API_KEY"] = "smoke-secret"
            env["GITHUB_ACTIONS"] = "true"
            completed = subprocess.run(
                [
                    sys.executable,
                    str(ROOT / "scripts/ci_diagnostics.py"),
                    "--root",
                    str(ROOT),
                    "--output",
                    str(output),
                ],
                cwd=ROOT,
                env=env,
                capture_output=True,
                text=True,
                check=True,
            )
            artifact_text = output.read_text(encoding="utf-8")
            artifact = json.loads(artifact_text)
            stdout_report = json.loads(completed.stdout)

        self.assertEqual(artifact, stdout_report)
        self.assertEqual(artifact["schema"], "simplicio.ci-diagnostics/v1")
        self.assertNotIn("smoke-secret", artifact_text)
        for command in artifact["commands"]:
            for field in ("stdout", "stderr", "error"):
                if field in command:
                    self.assertLessEqual(len(command[field]), 4000)


if __name__ == "__main__":
    unittest.main()
