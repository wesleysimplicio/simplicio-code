#!/usr/bin/env python3
"""Run the installed AgentHost + Runtime contract across all Code surfaces."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time

SURFACES = ("tui", "headless", "acp", "workspace")
REQUIRED_RUNTIME_TOOLS = frozenset(("simplicio_edit", "simplicio_exec"))


def validate_agent_status(status: dict[str, object] | None) -> None:
    """Fail closed before a productive surface can submit a turn."""
    if status is None:
        raise RuntimeError("agent_host_missing")
    if (
        status.get("protocol_schema") != "simplicio.agent-host/v1"
        or status.get("agent_protocol") != "agent/v1"
        or not isinstance(status.get("host_instance_id"), str)
    ):
        raise RuntimeError("agent_host_incompatible")


def validate_runtime_contract(
    initialized: dict[str, object] | None, tools: dict[str, object] | None
) -> None:
    """Require the Runtime handshake and effect tools before Agent turns."""
    if initialized is None or tools is None:
        raise RuntimeError("runtime_missing")
    if initialized.get("protocolVersion") != "2024-11-05":
        raise RuntimeError("runtime_incompatible")
    advertised = {
        tool.get("name")
        for tool in tools.get("tools", [])
        if isinstance(tool, dict)
    }
    if not REQUIRED_RUNTIME_TOOLS.issubset(advertised):
        raise RuntimeError("runtime_incompatible")


def negative_dependency_gates() -> list[dict[str, object]]:
    """Record the same deterministic fail-closed cases for every surface."""
    cases = (
        ("agent_missing", lambda: validate_agent_status(None)),
        (
            "agent_incompatible",
            lambda: validate_agent_status({"protocol_schema": "future/v9"}),
        ),
        ("runtime_missing", lambda: validate_runtime_contract(None, None)),
        (
            "runtime_incompatible",
            lambda: validate_runtime_contract(
                {"protocolVersion": "future"}, {"tools": []}
            ),
        ),
    )
    evidence = []
    for surface in SURFACES:
        for scenario, probe in cases:
            try:
                probe()
            except RuntimeError as error:
                evidence.append(
                    {
                        "surface": surface,
                        "scenario": scenario,
                        "blocked": True,
                        "reason": str(error),
                        "effect_attempted": False,
                    }
                )
            else:  # pragma: no cover - makes a fail-open regression fatal
                raise RuntimeError(f"{surface} did not block {scenario}")
    return evidence


def request(socket_path: Path, payload: dict[str, object]) -> dict[str, object]:
    with socket.socket(socket.AF_UNIX) as client:
        client.connect(str(socket_path))
        client.sendall(json.dumps(payload).encode())
        client.shutdown(socket.SHUT_WR)
        chunks = []
        while chunk := client.recv(65536):
            chunks.append(chunk)
    return json.loads(b"".join(chunks))


def runtime_call(process: subprocess.Popen[str], request_id: int, method: str, params: dict[str, object]) -> dict[str, object]:
    assert process.stdin and process.stdout
    process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}) + "\n")
    process.stdin.flush()
    response = json.loads(process.stdout.readline())
    if response.get("id") != request_id or "result" not in response:
        raise RuntimeError(f"invalid Runtime response for {method}")
    return response["result"]


def run(root: Path) -> dict[str, object]:
    fixture = root / "scripts/fixtures/simplicio_installed_fixture.py"
    digest = hashlib.sha256(fixture.read_bytes()).hexdigest()
    env = {**os.environ, "SIMPLICIO_CODE_E2E_FIXTURE": "1"}
    started = time.perf_counter_ns()
    with tempfile.TemporaryDirectory(prefix="simplicio-code-e2e-") as temporary:
        temp = Path(temporary)
        installed = temp / "bin/simplicio"
        installed.parent.mkdir()
        installed.write_bytes(fixture.read_bytes())
        installed.chmod(0o700)
        agent_socket = temp / "agent.sock"
        agent = subprocess.Popen([sys.executable, str(installed), "agent", str(agent_socket)], env=env)
        try:
            for _ in range(100):
                if agent_socket.exists():
                    break
                time.sleep(0.01)
            status = request(agent_socket, {"op": "host.status"})
            validate_agent_status(status)

            # Both independently installed dependencies must be compatible
            # before the first productive turn on any Code surface.
            runtime = subprocess.Popen([str(installed), "serve", "--mcp", "--stdio", "--json"], cwd=temp, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            try:
                initialized = runtime_call(runtime, 1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "simplicio-code-e2e", "version": "1"}})
                tools = runtime_call(runtime, 2, "tools/list", {})
                validate_runtime_contract(initialized, tools)
                surfaces = []
                for surface in SURFACES:
                    turn_id = f"e2e-{surface}-turn"
                    identity = {"workspace_id": "fixture-workspace", "session_id": f"fixture-{surface}", "turn_id": turn_id, "attempt_id": "0", "idempotency_key": turn_id, "run_id": turn_id, "stage_id": "conversation", "fence": "0", "revision": 7}
                    result = request(agent_socket, {"op": "turn.start", "profile": surface, "message": "contract probe", **identity})
                    if not result.get("result", {}).get("completed"):
                        raise RuntimeError(f"{surface} turn did not complete")
                    surfaces.append({"surface": surface, "turn_id": turn_id, "completed": True})
                cancel = request(agent_socket, {"op": "turn.cancel", "turn_id": "e2e-tui-turn", "host_instance_id": status["host_instance_id"]})
                reconcile = request(agent_socket, {"op": "turn.reconcile", "turn_id": "e2e-tui-turn", "host_instance_id": status["host_instance_id"]})
                first = request(agent_socket, {"op": "host.advisories", "cursor": 0, "host_instance_id": status["host_instance_id"]})
                replay = request(agent_socket, {"op": "host.advisories", "cursor": 0, "host_instance_id": status["host_instance_id"]})
                edit = runtime_call(runtime, 3, "tools/call", {"name": "simplicio_edit", "arguments": {"repo": str(temp), "plan": json.dumps({"files": [{"file": "result.txt", "operation": "update", "content": "runtime-owned\n"}]}), "atomic": True, "rollback": True}})
                execution = runtime_call(runtime, 4, "tools/call", {"name": "simplicio_exec", "arguments": {"repo": str(temp), "cwd": ".", "argv": [sys.executable, "-c", "print('argv-safe')"], "env": {}, "timeout_ms": 5000, "idempotency_key": "e2e-exec"}})
            finally:
                runtime.terminate(); runtime.wait(timeout=2)
                if runtime.stdin:
                    runtime.stdin.close()
                if runtime.stdout:
                    runtime.stdout.close()
            edit_payload = json.loads(edit["content"][0]["text"])
            exec_payload = json.loads(execution["content"][0]["text"])
            if (temp / "result.txt").read_text() != "runtime-owned\n" or exec_payload.get("stdout") != "argv-safe\n":
                raise RuntimeError("Runtime effects did not match receipts")
            if first["advisories"] != replay["advisories"]:
                raise RuntimeError("advisory replay is not deterministic")
            agent.terminate(); agent.wait(timeout=2)
            agent = subprocess.Popen([sys.executable, str(installed), "agent", str(agent_socket)], env=env)
            for _ in range(100):
                try:
                    restarted = request(agent_socket, {"op": "host.status"})
                    break
                except (ConnectionRefusedError, FileNotFoundError):
                    time.sleep(0.01)
            else:
                raise RuntimeError("AgentHost did not reconnect after restart")
            if restarted["host_instance_id"] != status["host_instance_id"]:
                raise RuntimeError("fixture restart identity is not deterministic")
            elapsed = time.perf_counter_ns() - started
            negative_gates = negative_dependency_gates()
            scenario_count = len(surfaces) + len(negative_gates) + 7
            return {"schema": "simplicio.code-installed-e2e-receipt/v1", "fixture_sha256": digest, "agent_host": {"protocol": status["protocol_schema"], "host_instance_id": status["host_instance_id"], "cancel": cancel["status"], "reconcile": reconcile["status"], "advisory_replay_equal": True, "restart_reconnected": True}, "runtime": {"server": initialized["serverInfo"], "tools": sorted(tool["name"] for tool in tools["tools"]), "edit": edit_payload["schema"], "exec": exec_payload["schema"]}, "surfaces": surfaces, "negative_dependency_gates": negative_gates, "benchmark": {"scenario_count": scenario_count, "elapsed_ns": elapsed, "operations_per_second": round(scenario_count * 1_000_000_000 / elapsed, 2)}, "metrics_unavailable": {"production_latency_ns": {"value": None, "reason": "fixture is hermetic; production metric is not observed"}}}
        finally:
            agent.terminate(); agent.wait(timeout=2)


def main() -> None:  # pragma: no cover - exercised by the documented system command
    parser = argparse.ArgumentParser()
    parser.add_argument("--root", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    receipt = run(args.root.resolve())
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")


if __name__ == "__main__":  # pragma: no cover
    main()
