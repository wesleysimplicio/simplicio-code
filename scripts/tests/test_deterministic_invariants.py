import tempfile
import unittest
from pathlib import Path

from scripts.check_deterministic_invariants import _initializer_body, check


class DeterministicInvariantTests(unittest.TestCase):
    def test_initializer_parser_handles_nested_braces(self) -> None:
        body = _initializer_body("let x = Example { field: Some({}), other: 1 };", "Example")
        self.assertIsNotNone(body)
        self.assertIn("field: Some({})", body or "")

    def test_missing_required_field_is_a_failure(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "crates/codegen/xai-grok-shell/src/session/acp_session_impl"
            path.mkdir(parents=True)
            (path / "spawn.rs").write_text("AgentRebuildSpec { fs_backend: x }", encoding="utf-8")
            findings = check(root)
            self.assertEqual({item["field"] for item in findings}, {"search_backend", "directory_backend"})


if __name__ == "__main__":
    unittest.main()
