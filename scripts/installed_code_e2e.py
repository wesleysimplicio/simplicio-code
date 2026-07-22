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
REQUIRED_AGENT_CAPABILITIES = {
    "host.advisories", "host.status", "turn.cancel", "turn.reconcile", "turn.start"
}


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
            if status.get("protocol_schema") != "simplicio.agent-host/v1" or status.get("agent_protocol") != "agent/v1":
                raise RuntimeError("AgentHost discovery contract mismatch")
            missing_capabilities = REQUIRED_AGENT_CAPABILITIES - set(status.get("capabilities", []))
            if missing_capabilities:
                raise RuntimeError(
                    f"AgentHost capabilities missing: {', '.join(sorted(missing_capabilities))}"
                )
            surfaces = []
            for index, surface in enumerate(SURFACES):
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

            runtime = subprocess.Popen([str(installed), "serve", "--mcp", "--stdio", "--json"], cwd=temp, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
            try:
                initialized = runtime_call(runtime, 1, "initialize", {"protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": {"name": "simplicio-code-e2e", "version": "1"}})
                tools = runtime_call(runtime, 2, "tools/list", {})
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
            evidence = {
                "fixture_sha256": digest,
                "agent_host": {
                    "protocol": status["protocol_schema"],
                    "agent_protocol": status["agent_protocol"],
                    "host_instance_id": status["host_instance_id"],
                    "capabilities": sorted(status["capabilities"]),
                    "cancel": cancel["status"],
                    "reconcile": reconcile["status"],
                    "advisory_replay_equal": True,
                    "restart_reconnected": True,
                },
                "runtime": {
                    "server": initialized["serverInfo"],
                    "tools": sorted(tool["name"] for tool in tools["tools"]),
                    "edit": edit_payload["schema"],
                    "edit_atomic": True,
                    "rollback_requested": True,
                    "rolled_back": edit_payload["rolled_back"],
                    "exec": exec_payload["schema"],
                    "exec_effect_state": exec_payload["effect_state"],
                },
                "surfaces": surfaces,
            }
            evidence_sha256 = hashlib.sha256(
                json.dumps(evidence, sort_keys=True, separators=(",", ":")).encode()
            ).hexdigest()
            return {
                "schema": "simplicio.code-installed-e2e-receipt/v1",
                "evidence_sha256": evidence_sha256,
                **evidence,
                "benchmark": {
                    "scenario_count": len(surfaces) + 7,
                    "elapsed_ns": elapsed,
                    "operations_per_second": round(
                        (len(surfaces) + 7) * 1_000_000_000 / elapsed, 2
                    ),
                },
                "metrics_unavailable": {
                    "production_latency_ns": {
                        "value": None,
                        "reason": "fixture is hermetic; production metric is not observed",
                    }
                },
            }
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
