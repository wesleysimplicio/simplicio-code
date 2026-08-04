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
    def test_fast_mode_parser_rejects_duplicate_and_unknown_modes(self):
        self.assertEqual(
            MODULE.parse_fast_modes("rust, python, off"),
            ("rust", "python", "off"),
        )
        with self.assertRaisesRegex(ValueError, "fast_mode_duplicate"):
            MODULE.parse_fast_modes("rust,python,rust")
        with self.assertRaisesRegex(ValueError, "fast_mode_unknown"):
            MODULE.parse_fast_modes("rust,wasm")

    def test_fast_fixture_matrix_has_explicit_not_executed_results(self):
        matrix = MODULE.build_fast_mode_matrix(
            MODULE.parse_fast_modes("rust,python,off"),
            fixture_mode=True,
        )
        self.assertEqual(matrix["schema"], MODULE.FAST_MATRIX_SCHEMA)
        self.assertEqual(
            matrix["requested_modes"], ["rust", "python", "off"]
        )
        self.assertEqual(
            [result["mode"] for result in matrix["results"]],
            ["rust", "python", "off"],
        )
        self.assertTrue(
            all(result["outcome"] == "not_executed" for result in matrix["results"])
        )
        self.assertTrue(
            all(not result["effect_attempted"] for result in matrix["results"])
        )
        self.assertTrue(
            all(not result["local_llm_started"] for result in matrix["results"])
        )

    def test_fast_probe_receipt_records_ready_and_blocked_outcomes(self):
        def fake_runner(command, **kwargs):
            mode = command[2]
            if mode == "rust":
                return MODULE.subprocess.CompletedProcess(
                    command,
                    1,
                    stdout=json.dumps(
                        {
                            "selected_engine": "unavailable",
                            "reason": "rust_executable_missing",
                        },
                        indent=2,
                    ),
                    stderr="",
                )
            return MODULE.subprocess.CompletedProcess(
                command,
                0,
                stdout=json.dumps(
                    {"selected_engine": "python", "version": "2.0.24"},
                    indent=2,
                ),
                stderr="",
            )

        matrix = MODULE.build_fast_mode_matrix(
            ("rust", "python"),
            executable="simplicio-fast",
            runner=fake_runner,
        )
        self.assertEqual(
            [result["outcome"] for result in matrix["results"]],
            ["blocked", "ready"],
        )
        self.assertEqual(matrix["results"][1]["version"], "2.0.24")
        self.assertTrue(
            all(not result["local_llm_started"] for result in matrix["results"])
        )
        self.assertEqual(
            json.loads(json.dumps(matrix))["schema"],
            MODULE.FAST_MATRIX_SCHEMA,
        )

    def test_installed_fixture_covers_every_productive_surface_and_effect(self):
        receipt = MODULE.run(ROOT, fixture_mode=True)
        self.assertEqual(receipt["schema"], "simplicio.code-installed-e2e-receipt/v1")
        self.assertEqual(receipt["proof_kind"], "hermetic_fixture_non_proof")
        component_manifest = receipt["component_manifest"]
        self.assertEqual(
            component_manifest["schema"], "simplicio.installed-components/v1"
        )
        self.assertEqual(
            component_manifest["proof_kind"], "hermetic_fixture_non_proof"
        )
        self.assertEqual(component_manifest["surfaces"], list(MODULE.SURFACES))
        self.assertEqual(
            component_manifest["runtime"]["protocol_version"], "2024-11-05"
        )
        self.assertIn("simplicio_edit", component_manifest["runtime"]["tools"])
        components = {
            item["role"]: item for item in component_manifest["components"]
        }
        self.assertEqual(
            set(components),
            {"code", "agent_host", "runtime", "loop_hub", "mapper", "dev_cli", "fast"},
        )
        self.assertEqual(
            components["agent_host"]["proof_kind"], "hermetic_fixture_non_proof"
        )
        self.assertEqual(
            components["runtime"]["version"], "code-e2e-fixture/1"
        )
        for component in components.values():
            self.assertIn("status", component)
            self.assertIn("sha256", component)
        process_observations = component_manifest["process_observations"]
        self.assertEqual(
            process_observations["schema"], "simplicio.process-observations/v1"
        )
        self.assertTrue(process_observations["independent"])
        self.assertTrue(process_observations["restart"]["rotated"])
        self.assertEqual(
            {item["role"] for item in process_observations["processes"]},
            {"agent_host", "runtime"},
        )
        expected_agent_transport = (
            "loopback_tcp" if MODULE.os.name == "nt" else "unix_socket"
        )
        self.assertEqual(
            {item["transport"] for item in process_observations["processes"]},
            {expected_agent_transport, "stdio"},
        )
        self.assertEqual(
            [item["surface"] for item in receipt["surfaces"]], list(MODULE.SURFACES)
        )
        self.assertEqual(receipt["agent_host"]["cancel"], "cancelled")
        self.assertEqual(receipt["agent_host"]["reconcile"], "terminal")
        self.assertTrue(receipt["agent_host"]["advisory_replay_equal"])
        self.assertTrue(receipt["agent_host"]["restart_reconnected"])
        self.assertEqual(receipt["mode"], "fixture")
        self.assertEqual(receipt["runtime"]["map"], "simplicio.map-result/v1")
        self.assertEqual(receipt["runtime"]["read"], "simplicio.read-result/v1")
        self.assertEqual(receipt["runtime"]["edit"], "simplicio.edit-result/v1")
        self.assertIn(
            receipt["runtime"]["exec"],
            {"simplicio.exec-result/v1", "simplicio.release-manifest/v1"},
        )
        self.assertEqual(receipt["runtime"]["test_run"], "simplicio.test-run/v1")
        self.assertEqual(receipt["runtime"]["effect_state"], "completed")
        self.assertTrue(receipt["runtime"]["restart"]["reconnected"])
        self.assertTrue(receipt["runtime"]["restart_tools_match"])
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
                    {"protocolVersion": "2024-11-05", "capabilities": {"runtime_process": {name: True for name in MODULE.REQUIRED_RUNTIME_PROCESS_CAPABILITIES}}}, {"tools": []}
                ),
                "runtime_incompatible",
            ),
        )
        for probe, reason in cases:
            with self.subTest(reason=reason), self.assertRaisesRegex(
                RuntimeError, reason
            ):
                probe()


    def test_missing_runtime_process_capability_fails_closed(self):
        initialized = {"protocolVersion": "2024-11-05", "capabilities": {"runtime_process": {"start": True}}}
        with self.assertRaisesRegex(RuntimeError, "runtime_process_incompatible"):
            MODULE.validate_runtime_process_capabilities(initialized)

    def test_runtime_36_contract_matches_current_mcp_tools(self):
        initialized = {"protocolVersion": "2024-11-05", "capabilities": {"runtime_process": {name: True for name in MODULE.REQUIRED_RUNTIME_PROCESS_CAPABILITIES}}}
        tools = {
            "tools": [{"name": name} for name in MODULE.REQUIRED_RUNTIME_TOOLS]
        }
        MODULE.validate_runtime_contract(initialized, tools)


    def test_runtime_release_identity_records_redacted_manifest_and_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "simplicio.exe"
            executable.write_bytes(b"runtime-binary")
            payload = {
                "schema": MODULE.RUNTIME_RELEASE_SCHEMA,
                "binary": "simplicio",
                "runtime": {
                    "name": "simplicio-runtime",
                    "version": "3.6.0",
                    "commit": "abc123",
                    "target": "windows-x86_64",
                },
                "capabilities": ["serve-mcp", "edit"],
                "secret": "must-not-escape",
            }

            def runner(command, **kwargs):
                return MODULE.subprocess.CompletedProcess(
                    command, 0, json.dumps(payload), ""
                )

            identity = MODULE.read_runtime_release_identity(
                str(executable), runner=runner
            )
        self.assertEqual(identity["version"], "3.6.0")
        self.assertEqual(identity["capabilities"], ["edit", "serve-mcp"])
        self.assertEqual(
            identity["sha256"],
            MODULE.hashlib.sha256(b"runtime-binary").hexdigest(),
        )
        self.assertNotIn("must-not-escape", json.dumps(identity))


    def test_runtime_release_identity_rejects_incompatible_manifest(self):
        with tempfile.TemporaryDirectory() as directory:
            executable = Path(directory) / "simplicio.exe"
            executable.write_bytes(b"runtime-binary")

            def runner(command, **kwargs):
                return MODULE.subprocess.CompletedProcess(
                    command,
                    0,
                    json.dumps({"schema": "future/v9", "capabilities": []}),
                    "",
                )

            with self.assertRaisesRegex(
                RuntimeError, "release manifest missing version/capabilities"
            ):
                MODULE.read_runtime_release_identity(str(executable), runner=runner)

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

    def test_external_mode_rejects_wrapper_without_executable_target(self):
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as directory:
            wrapper = Path(directory) / "agent.cmd"
            missing_target = Path(directory) / "missing-hermes.exe"
            wrapper.write_text(
                '@echo off' + chr(10) + f'"{missing_target}" %*' + chr(10),
                encoding="utf-8",
            )
            wrapper.chmod(0o755)
            with patch.dict(
                os.environ,
                {
                    "SIMPLICIO_AGENT_HOST_E2E_COMMAND": json.dumps([str(wrapper)]),
                    "SIMPLICIO_RUNTIME_BIN": MODULE.sys.executable,
                },
                clear=False,
            ):
                with self.assertRaisesRegex(
                    RuntimeError, "wrapper target is not executable"
                ):
                    MODULE.run(ROOT)

    def test_installed_dependency_diagnosis_is_effect_free(self):
        import os
        from unittest.mock import patch

        with tempfile.TemporaryDirectory() as directory:
            wrapper = Path(directory) / "agent.cmd"
            missing_target = Path(directory) / "missing-hermes.exe"
            wrapper.write_text(
                '@echo off' + chr(10) + f'"{missing_target}" %*' + chr(10),
                encoding="utf-8",
            )
            wrapper.chmod(0o755)
            with patch.dict(
                os.environ,
                {
                    "SIMPLICIO_AGENT_HOST_E2E_COMMAND": json.dumps([str(wrapper)]),
                    "SIMPLICIO_RUNTIME_BIN": MODULE.sys.executable,
                },
                clear=False,
            ):
                diagnosis = MODULE.diagnose_installed_dependencies()
        self.assertEqual(
            diagnosis["schema"], "simplicio.installed-dependency-diagnostic/v1"
        )
        self.assertEqual(diagnosis["status"], "blocked")
        self.assertFalse(diagnosis["effect_attempted"])
        self.assertFalse(diagnosis["productive_flow_verified"])
        self.assertIn("wrapper target is not executable", diagnosis["reason"])

    def test_tcp_sidecar_requires_loopback_and_authentication(self):
        with tempfile.TemporaryDirectory() as directory:
            socket_path = Path(directory) / "agent.sock"
            endpoint_path = socket_path.with_suffix(".tcp")
            token_path = socket_path.with_suffix(".token")
            endpoint_path.write_text("127.0.0.1:4242", encoding="ascii")
            token_path.write_text("t" * 32, encoding="ascii")
            self.assertEqual(
                MODULE.read_tcp_endpoint(socket_path),
                ("127.0.0.1", 4242, "t" * 32),
            )
            endpoint_path.write_text("192.0.2.1:4242", encoding="ascii")
            with self.assertRaisesRegex(RuntimeError, "not_loopback"):
                MODULE.read_tcp_endpoint(socket_path)
            endpoint_path.write_text("127.0.0.1:4242", encoding="ascii")
            token_path.unlink()
            with self.assertRaisesRegex(RuntimeError, "auth_missing"):
                MODULE.read_tcp_endpoint(socket_path)

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
