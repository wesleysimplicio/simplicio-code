from pathlib import Path
import unittest


WORKFLOW = Path(__file__).parents[2] / ".github" / "workflows" / "release.yml"


class ReleaseWorkflowTests(unittest.TestCase):
    def test_publish_resolves_version_in_its_own_job(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        publish = text.split("  publish:\n", 1)[1]
        self.assertIn("id: ver", publish)
        self.assertIn('echo "version=${GITHUB_REF_NAME#v}"', publish)
        self.assertIn("body_path: RELEASE_NOTES_${{ steps.ver.outputs.version }}.md", publish)

    def test_release_workflow_does_not_claim_production_signing(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        self.assertIn("NOT a production trust root", text)

    def test_windows_release_target_is_required(self) -> None:
        text = WORKFLOW.read_text(encoding="utf-8")
        windows = text.split("platform: windows-x86_64", 1)[1].split("    runs-on:", 1)[0]
        self.assertIn("best_effort: false", windows)


if __name__ == "__main__":
    unittest.main()
