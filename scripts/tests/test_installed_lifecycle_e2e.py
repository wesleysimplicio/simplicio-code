import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("installed_lifecycle_e2e", ROOT / "scripts/release/installed_lifecycle_e2e.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
OLD = ROOT / "scripts/fixtures/lifecycle/v1/simplicio-code"
NEW = ROOT / "scripts/fixtures/lifecycle/v2/simplicio-code"


class InstalledLifecycleE2ETest(unittest.TestCase):
    def test_clean_install_upgrade_rollback_is_reproducible(self):
        receipts = []
        for _ in range(2):
            with tempfile.TemporaryDirectory() as parent:
                receipts.append(MODULE.run(Path(parent) / "install", OLD, NEW, "fixture"))
        self.assertEqual(receipts[0], receipts[1])
        receipt = receipts[0]
        self.assertEqual(receipt["status"], "passed")
        self.assertEqual(receipt["clean_install"]["observed"]["probe"], "lifecycle-fixture-v1")
        self.assertEqual(receipt["upgrade"]["observed"]["probe"], "lifecycle-fixture-v2")
        self.assertEqual(receipt["rollback"]["observed"], receipt["clean_install"]["observed"])
        self.assertFalse(receipt["issue_closure_claimed"])
        self.assertIsNone(receipt["unobserved"]["production_release"]["value"])
        self.assertNotIn(tempfile.gettempdir(), json.dumps(receipt))

    def test_missing_artifact_and_dirty_prefix_fail_closed(self):
        with tempfile.TemporaryDirectory() as parent:
            with self.assertRaisesRegex(MODULE.LifecycleRejected, "new_artifact_missing"):
                MODULE.run(Path(parent) / "install", OLD, Path(parent) / "missing", "explicit")
            dirty = Path(parent) / "dirty"
            dirty.mkdir()
            with self.assertRaisesRegex(MODULE.LifecycleRejected, "clean_prefix_already_exists"):
                MODULE.run(dirty, OLD, NEW, "fixture")

    def test_non_executable_and_incomplete_discovery_fail_closed(self):
        with tempfile.TemporaryDirectory() as parent:
            inert = Path(parent) / "inert"
            inert.write_text("not executable")
            with self.assertRaisesRegex(MODULE.LifecycleRejected, "old_artifact_not_executable"):
                MODULE.run(Path(parent) / "install", inert, NEW, "explicit")
        with patch.dict("os.environ", {"PATH": "", "SIMPLICIO_CODE_INSTALLED_BIN": "", "SIMPLICIO_CODE_UPGRADE_BIN": ""}, clear=False):
            with self.assertRaisesRegex(MODULE.LifecycleRejected, "installed_artifact_missing"):
                MODULE.discover(None, None)


if __name__ == "__main__":
    unittest.main()
