import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "installed_code_e2e", ROOT / "scripts/installed_code_e2e.py"
)
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)
FIXTURE_SPEC = importlib.util.spec_from_file_location(
    "simplicio_installed_fixture",
    ROOT / "scripts/fixtures/simplicio_installed_fixture.py",
)
FIXTURE = importlib.util.module_from_spec(FIXTURE_SPEC)
FIXTURE_SPEC.loader.exec_module(FIXTURE)


class InstalledCodeE2ETest(unittest.TestCase):
    def test_installed_fixture_covers_every_productive_surface_and_effect(self):
        receipt = MODULE.run(ROOT, fixture_mode=True)
        self.assertEqual(receipt["schema"], "simplicio.code-installed-e2e-receipt/v1")
        self.assertEqual(receipt["proof_kind"], "hermetic_fixture_non_proof")
        self.assertEqual(
            [item["surface"] for item in receipt["surfaces"]], list(MODULE.SURFACES)
        )
        self.assertEqual(receipt["agent_host"]["cancel"], "cancelled")
        self.assertEqual(receipt["agent_host"]["reconcile"], "terminal")
        self.assertTrue(receipt["agent_host"]["advisory_replay_equal"])
        self.assertTrue(receipt["agent_host"]["restart_reconnected"])
        self.assertEqual(receipt["mode"], "fixture")
        self.assertEqual(receipt["runtime"]["list"], "simplicio.fs-list-result/v1")
        self.assertEqual(receipt["runtime"]["stat"], "simplicio.fs-stat-result/v1")
        self.assertEqual(receipt["runtime"]["edit"], "simplicio.edit-result/v1")
        self.assertEqual(receipt["runtime"]["exec"], "simplicio.exec-result/v1")
        self.assertEqual(receipt["runtime"]["effect_state"], "completed")
        self.assertTrue(receipt["runtime"]["prototype_artifact_idempotent_retry"])
        gates = receipt["negative_dependency_gates"]
        self.assertEqual(len(gates), len(MODULE.SURFACES) * 4)
        self.assertTrue(all(gate["blocked"] for gate in gates))
        self.assertTrue(all(not gate["effect_attempted"] for gate in gates))
        self.assertEqual({gate["surface"] for gate in gates}, set(MODULE.SURFACES))
        self.assertGreater(receipt["benchmark"]["operations_per_second"], 0)
        self.assertTrue(receipt["profile_isolation"])

    def test_dependency_contract_fails_closed_before_productive_turns(self):
        cases = (
            (lambda: MODULE.validate_agent_status(None), "agent_host_missing"),
            (
                lambda: MODULE.validate_agent_status(
                    {"protocol_schema": "simplicio.agent-host/v1"}
                ),
                "agent_host_incompatible",
            ),
            (lambda: MODULE.validate_runtime_contract(None, None), "runtime_missing"),
            (
                lambda: MODULE.validate_runtime_contract(
                    {"protocolVersion": "2024-11-05"}, {"tools": []}
                ),
                "runtime_incompatible",
            ),
        )
        for probe, reason in cases:
            with self.subTest(reason=reason), self.assertRaisesRegex(
                RuntimeError, reason
            ):
                probe()

    def test_explicit_installed_mode_never_falls_back_to_fixture(self):
        with tempfile.TemporaryDirectory() as directory:
            missing = Path(directory) / "missing-simplicio"
            with self.assertRaisesRegex(RuntimeError, "installed_binary_unavailable"):
                MODULE.run(ROOT, missing)

    def test_receipt_is_serializable_and_has_no_environment(self):
        receipt = MODULE.run(ROOT, fixture_mode=True)
        encoded = json.dumps(receipt)
        self.assertNotIn("HOME", encoded)
        self.assertNotIn("TOKEN", encoded)
        self.assertNotIn("Authorization", encoded)
        self.assertNotIn("Bearer ", encoded)
        metric = receipt["metrics_unavailable"]["production_latency_ns"]
        self.assertIsNone(metric["value"])
        self.assertEqual(
            metric["reason"], "fixture is hermetic; production metric is not observed"
        )

    def test_external_mode_fails_closed_without_installed_dependencies(self):
        import os
        from unittest.mock import patch

        with patch.dict(
            os.environ,
            {
                "PATH": "",
                "SIMPLICIO_AGENT_HOST_E2E_COMMAND": "",
                "SIMPLICIO_RUNTIME_BIN": "",
            },
            clear=False,
        ):
            with self.assertRaisesRegex(RuntimeError, "agent_host_missing"):
                MODULE.run(ROOT)

    def test_fixture_rejects_invalid_identity_and_path_escape(self):
        rejected = FIXTURE.agent_response({"op": "turn.start", "turn_id": "one"}, {})
        self.assertFalse(rejected["ok"])
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "escapes repository"):
                FIXTURE._safe_path(Path(directory).resolve(), "../outside")

    def test_fixture_unit_contract_supports_status_cancel_and_runtime_effects(self):
        state = {}
        status = FIXTURE.agent_response({"op": "host.status"}, state)
        self.assertTrue(status["host"]["ready"])
        self.assertEqual(
            FIXTURE.agent_response({"op": "turn.cancel", "turn_id": "missing"}, state)[
                "status"
            ],
            "not_found",
        )
        identity = {
            "workspace_id": "w",
            "session_id": "s",
            "turn_id": "t",
            "attempt_id": "0",
            "idempotency_key": "t",
            "run_id": "r",
            "stage_id": "stage",
            "fence": "0",
            "revision": 1,
        }
        turn = FIXTURE.agent_response(
            {"op": "turn.start", "profile": "tui", **identity}, state
        )
        self.assertTrue(turn["result"]["completed"])
        self.assertEqual(
            FIXTURE.agent_response({"op": "turn.cancel", "turn_id": "t"}, state)[
                "status"
            ],
            "cancelled",
        )
        self.assertEqual(
            FIXTURE.agent_response({"op": "turn.reconcile", "turn_id": "t"}, state)[
                "status"
            ],
            "terminal",
        )
        self.assertEqual(
            FIXTURE.agent_response(
                {"op": "turn.reconcile", "turn_id": "absent"}, state
            )["status"],
            "not_found",
        )
        self.assertEqual(
            len(
                FIXTURE.agent_response({"op": "host.advisories", "cursor": 0}, state)[
                    "advisories"
                ]["events"]
            ),
            1,
        )
        self.assertEqual(
            FIXTURE.agent_response({"op": "host.advisories", "cursor": 1}, state)[
                "advisories"
            ]["events"],
            [],
        )
        self.assertFalse(FIXTURE.agent_response({"op": "unsupported"}, state)["ok"])
        with tempfile.TemporaryDirectory() as directory:
            result = FIXTURE.runtime_tool(
                "simplicio_edit",
                {
                    "repo": directory,
                    "plan": json.dumps(
                        {"files": [{"file": "nested/result", "content": "ok"}]}
                    ),
                },
            )
            self.assertFalse(result["isError"])
            self.assertEqual((Path(directory) / "nested/result").read_text(), "ok")
            generic = FIXTURE.runtime_tool("simplicio_search", {"repo": directory})
            self.assertFalse(generic["isError"])
            executed = FIXTURE.runtime_tool(
                "simplicio_exec",
                {
                    "repo": directory,
                    "cwd": ".",
                    "argv": ["python3", "-c", "print('unit')"],
                    "env": {},
                    "timeout_ms": 5000,
                },
            )
            self.assertEqual(
                json.loads(executed["content"][0]["text"])["stdout"], "unit\n"
            )


if __name__ == "__main__":
    unittest.main()
