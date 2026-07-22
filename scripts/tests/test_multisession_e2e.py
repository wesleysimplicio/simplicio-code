import importlib.util
import json
from pathlib import Path
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("multisession_e2e", ROOT / "scripts/multisession_e2e.py")
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def valid_trace():
    states = ["working", "waiting", "blocked", "done"]
    return {
        "authority": {"decision_owner": "external_invoking_llm", "internal_provider_started": False,
                      "local_llm_started": False, "orca_opened": False},
        "credentials": "available",
        "dependencies": {name: "installed" for name in ("agent_host", "runtime", "loop_hub", "mapper")},
        "sessions": [{"session_id": f"session-{i}", "state": states[i % 4], "identity_preserved": True}
                     for i in range(20)],
        "surfaces": [{"surface": surface, "status": "passed", "semantic_hash": "same"}
                     for surface in MODULE.SURFACES],
        "worktrees": {"concurrent_paths": ["/work/a", "/work/b"], "collision_detected": True,
                      "overwrite_prevented": True},
        "recovery": {"restart": True, "reconnect": True, "cancel": True, "replay": True,
                     "unknown_effect_reconciled": True, "duplicate_effects": 0},
        "governance": {"implementer": "agent-a", "reviewer": "agent-b", "approver": "coordinator",
                       "prototype_first": True, "final_e2e_approved": False},
        "delivery": {"confirmed_count": 1, "remote_requery": True, "receipt_hash": "abc"},
        "benchmark_samples": [{name: value for name in MODULE.METRICS} for value in (10, 20, 30)],
    }


def classify(value):
    raw = json.dumps(value, sort_keys=True).encode()
    return MODULE.classify(value, raw)


class MultisessionE2ETest(unittest.TestCase):
    def test_complete_evidence_is_only_ready_for_coordinator(self):
        receipt = classify(valid_trace())
        self.assertEqual(receipt["status"], "READY_FOR_COORDINATOR_APPROVAL")
        self.assertFalse(receipt["final_e2e_approved"])
        self.assertEqual(receipt["benchmark"]["latency_ms"]["p50"], 20)
        self.assertEqual(receipt["benchmark"]["latency_ms"]["p95"], 30)

    def test_missing_dependency_and_metric_never_win(self):
        trace = valid_trace()
        trace["dependencies"]["runtime"] = "missing"
        del trace["benchmark_samples"][0]["cost"]
        del trace["benchmark_samples"][1]["cost"]
        del trace["benchmark_samples"][2]["cost"]
        receipt = classify(trace)
        self.assertEqual(receipt["status"], "BLOCKED")
        self.assertIn("runtime is not installed", receipt["blocked_reasons"])
        self.assertIsNone(receipt["benchmark"]["cost"]["p50"])

    def test_missing_credentials_is_blocked(self):
        trace = valid_trace(); trace["credentials"] = "missing"
        self.assertEqual(classify(trace)["status"], "BLOCKED")

    def test_external_authority_and_no_local_provider_are_fail_closed(self):
        for field, value in (("decision_owner", "internal"), ("internal_provider_started", True),
                             ("local_llm_started", True), ("orca_opened", True)):
            trace = valid_trace(); trace["authority"][field] = value
            with self.subTest(field=field):
                self.assertEqual(classify(trace)["status"], "FAILED")

    def test_identity_collision_recovery_and_governance_failures(self):
        mutations = [
            lambda t: t["sessions"].__setitem__(1, {**t["sessions"][1], "session_id": "session-0"}),
            lambda t: t["worktrees"].__setitem__("overwrite_prevented", False),
            lambda t: t["recovery"].__setitem__("duplicate_effects", 1),
            lambda t: t["governance"].__setitem__("reviewer", "agent-a"),
            lambda t: t["delivery"].__setitem__("confirmed_count", 2),
        ]
        for mutate in mutations:
            trace = valid_trace(); mutate(trace)
            self.assertEqual(classify(trace)["status"], "FAILED")

    def test_privacy_scan_rejects_secret_patterns(self):
        trace = valid_trace(); trace["unsafe"] = "Authorization: Bearer do-not-persist"
        receipt = classify(trace)
        self.assertEqual(receipt["status"], "FAILED")
        self.assertTrue(any("privacy" in error for error in receipt["errors"]))

    def test_cli_rejects_invalid_input_and_does_not_approve(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "trace.json"; path.write_text("[]")
            self.assertEqual(MODULE.main([str(path)]), 2)
            path.write_text(json.dumps(valid_trace()))
            output = Path(directory) / "receipt.json"
            self.assertEqual(MODULE.main([str(path), "--output", str(output)]), 0)
            self.assertFalse(json.loads(output.read_text())["final_e2e_approved"])


if __name__ == "__main__":
    unittest.main()
