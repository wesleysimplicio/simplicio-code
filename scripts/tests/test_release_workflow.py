import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

SCRIPT = Path(__file__).parents[2] / "scripts/release/generate_manifest.py"


WORKFLOW = Path(__file__).parents[2] / ".github" / "workflows" / "release.yml"


class ReleaseWorkflowTests(unittest.TestCase):
    def test_publish_resolves_version_in_its_own_job(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        publish = text.split("  publish:\n", 1)[1]
        self.assertIn("id: ver", publish)
        self.assertIn('echo "version=${GITHUB_REF_NAME#v}"', publish)
        self.assertIn("body_path: RELEASE_NOTES_${{ steps.ver.outputs.version }}.md", publish)

    def test_manifest_records_source_commit(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        package = text.split("      - name: Generate signed release manifest", 1)[1].split("  publish:\n", 1)[0]
        self.assertIn('--commit-sha \"${{ github.sha }}\"', package)

    def test_release_workflow_does_not_claim_production_signing(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("NOT a production trust root", text)

    def test_windows_release_target_is_required(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        windows = text.split("platform: windows-x86_64", 1)[1].split("    runs-on:", 1)[0]
        self.assertIn("best_effort: false", windows)


    def test_generator_requires_every_declared_release_platform(self):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            for platform, suffix in (
                ("linux-x86_64", ""),
                ("macos-aarch64", ""),
                ("windows-x86_64", ".exe"),
            ):
                (artifacts / f"simplicio-code-0.3.0-beta.4-{platform}{suffix}").write_bytes(
                    platform.encode("ascii")
                )
            output = artifacts / "manifest.json"
            command = [
                sys.executable,
                str(SCRIPT),
                "--version",
                "0.3.0-beta.4",
                "--channel",
                "beta",
                "--commit-sha",
                "a" * 40,
                "--artifacts-dir",
                str(artifacts),
                "--out",
                str(output),
            ]
            for platform in ("linux-x86_64", "macos-aarch64", "windows-x86_64"):
                command.extend(("--required-platform", platform))
            result = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)
            manifest = json.loads(output.read_text(encoding="utf-8"))
            self.assertEqual(
                {entry["platform"] for entry in manifest["artifacts"]},
                {"linux-x86_64", "macos-aarch64", "windows-x86_64"},
            )

    def test_generator_rejects_a_missing_declared_platform(self):
        with tempfile.TemporaryDirectory() as directory:
            artifacts = Path(directory)
            (artifacts / "simplicio-code-0.3.0-beta.4-linux-x86_64").write_bytes(b"linux")
            output = artifacts / "manifest.json"
            result = subprocess.run(
                [
                    sys.executable,
                    str(SCRIPT),
                    "--version",
                    "0.3.0-beta.4",
                    "--channel",
                    "beta",
                    "--commit-sha",
                    "a" * 40,
                    "--artifacts-dir",
                    str(artifacts),
                    "--out",
                    str(output),
                    "--required-platform",
                    "linux-x86_64",
                    "--required-platform",
                    "windows-x86_64",
                ],
                capture_output=True,
                text=True,
                check=False,
            )
        self.assertNotEqual(result.returncode, 0)
        self.assertIn(
            "missing required release platform(s): windows-x86_64", result.stderr
        )
    def test_release_workflow_blocks_placeholder_trust_root_for_stable(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        package = text.split(
            "      - name: Generate signed release manifest", 1
        )[1].split("  publish:", 1)[0]
        self.assertIn(
            'if [ "${{ steps.ver.outputs.channel }}" = "stable" ]; then',
            package,
        )
        self.assertIn("production trust root required for stable release", package)
    def test_release_workflow_rejects_historical_beta5(self):
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn(
            'if [ "${{ steps.ver.outputs.version }}" = "0.3.0-beta.5" ]; then',
            text,
        )
        self.assertIn("historical beta.5 is not publishable", text)


if __name__ == "__main__":
    unittest.main()
