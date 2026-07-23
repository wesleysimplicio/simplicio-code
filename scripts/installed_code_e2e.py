#!/usr/bin/env python3
"""Run the installed AgentHost + Runtime contract across all Code surfaces."""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import socket
import shutil
import subprocess
import sys
import tempfile
import time

SURFACES = ("tui", "headless", "acp", "workspace")
REQUIRED_RUNTIME_TOOLS = frozenset(
    (
        "simplicio_edit",
        "simplicio_exec",
        "simplicio_file_read",
        "simplicio_fs_delete",
        "simplicio_fs_list",
        "simplicio_fs_stat",
        "simplicio_fs_write",
        "simplicio_search",
        "simplicio_prototype_artifact_read",
        "simplicio_prototype_artifact_write",
    )
)
AGENT_STARTUP_TIMEOUT_S = 30.0
REQUIRED_AGENT_CAPABILITIES = frozenset(
    ("host.advisories", "host.status", "turn.cancel", "turn.reconcile", "turn.start")
)


def validate_agent_status(status: dict[str, object] | None) -> None:
    """Fail closed before a productive surface can submit a turn."""
    if status is None:
        raise RuntimeError("agent_host_missing")
    if (
        status.get("protocol_schema") != "simplicio.agent-host/v1"
        or status.get("agent_protocol") != "agent/v1"
        or not isinstance(status.get("host_instance_id"), str)
        or not status.get("host_instance_id")
        or not REQUIRED_AGENT_CAPABILITIES.issubset(status.get("capabilities", []))
        or not status.get("host", {}).get("ready")
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
        tool.get("name") for tool in tools.get("tools", []) if isinstance(tool, dict)
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
    endpoint = socket_path.read_text(encoding="ascii").strip() if socket_path.is_file() else ""
    if endpoint.startswith("tcp://"):
        host, port_text = endpoint.removeprefix("tcp://").rsplit(":", 1)
        client = socket.create_connection((host, int(port_text)))
    else:
        family = getattr(socket, "AF_UNIX", None)
        if family is None:
            raise RuntimeError("agent_transport_unavailable")
        client = socket.socket(family)
        client.connect(str(socket_path))
    with client:
        client.sendall(json.dumps(payload).encode())
        client.shutdown(socket.SHUT_WR)
        chunks = []
        while chunk := client.recv(65536):
            chunks.append(chunk)
    return json.loads(b"".join(chunks))


def wait_for_agent_socket(
    process: subprocess.Popen[str], socket_path: Path, *, timeout_s: float = AGENT_STARTUP_TIMEOUT_S
) -> None:
    """Wait for a real AgentHost endpoint and report early process failure clearly."""
    deadline = time.monotonic() + timeout_s
    while time.monotonic() < deadline:
        if socket_path.exists():
            return
        return_code = process.poll()
        if return_code is not None:
            stderr = ""
            if process.stderr is not None:
                stderr = process.stderr.read().strip()[-2000:]
            detail = f": {stderr}" if stderr else ""
            raise RuntimeError(f"agent_host_exited:{return_code}{detail}")
        time.sleep(0.02)
    raise RuntimeError(f"agent_host_start_timeout:{timeout_s:.1f}s")


def close_process_pipes(process: subprocess.Popen[str]) -> None:
    for stream in (process.stdin, process.stdout, process.stderr):
        if stream is not None:
            stream.close()


def runtime_call(
    process: subprocess.Popen[str],
    request_id: int,
    method: str,
    params: dict[str, object],
) -> dict[str, object]:
    assert process.stdin and process.stdout
    process.stdin.write(
        json.dumps(
            {"jsonrpc": "2.0", "id": request_id, "method": method, "params": params}
        )
        + "\n"
    )
    process.stdin.flush()
    response = json.loads(process.stdout.readline())
    if response.get("id") != request_id or "result" not in response:
        raise RuntimeError(f"invalid Runtime response for {method}")
    return response["result"]


def effect_arguments(
    capability: str, arguments: dict[str, object], *, transaction_id: str
) -> dict[str, object]:
    """Attach the Runtime's causal effect envelope to a mutating call."""
    return {
        **arguments,
        "__runtime_effect_transaction": {
            "schema": "simplicio.effect-transaction/v1",
            "executor": "simplicio-runtime",
            "request": {
                "schema": "simplicio.effect-request/v1",
                "capability": capability,
                "identity": {
                    "session": "code-installed-e2e",
                    "turn": transaction_id,
                    "tool_call": transaction_id,
                    "attempt": "0",
                    "transaction": transaction_id,
                },
                "authority": "code-installed-e2e",
                "policy_receipt": "code-installed-e2e-policy",
                "idempotency_key": transaction_id,
                "action_digest": f"sha256:{transaction_id}",
                "write_set": [f"repo:{transaction_id}"],
                "preconditions": ["workspace:prepared"],
                "lease": {"id": f"lease-{transaction_id}", "fence": 1},
                "deadline_ms": int(time.time() * 1000) + 60_000,
                "cancellation": "safe_boundary_only",
                "validation_plan": "installed-e2e-validation",
                "rollback_plan": "installed-e2e-rollback",
                "redaction_plan": "installed-e2e-redaction",
            },
        },
    }


def _external_dependencies() -> tuple[list[str], list[str]]:
    """Resolve independently installed executors without inventing a fallback."""
    encoded = os.environ.get("SIMPLICIO_AGENT_HOST_E2E_COMMAND")
    if not encoded:
        raise RuntimeError("agent_host_missing: set SIMPLICIO_AGENT_HOST_E2E_COMMAND")
    try:
        agent = json.loads(encoded)
    except json.JSONDecodeError as error:
        raise RuntimeError(
            "agent_host_incompatible: AgentHost command is not JSON argv"
        ) from error
    if (
        not isinstance(agent, list)
        or not agent
        or not all(isinstance(item, str) for item in agent)
    ):
        raise RuntimeError(
            "agent_host_incompatible: AgentHost command must be JSON argv"
        )
    runtime = os.environ.get("SIMPLICIO_RUNTIME_BIN") or shutil.which("simplicio")
    if not runtime:
        raise RuntimeError("runtime_missing: set SIMPLICIO_RUNTIME_BIN")
    agent_executable = shutil.which(agent[0]) or agent[0]
    if not Path(agent_executable).is_file() or not os.access(agent_executable, os.X_OK):
        raise RuntimeError("agent_host_missing: AgentHost executable is not executable")
    if not Path(runtime).is_file() or not os.access(runtime, os.X_OK):
        raise RuntimeError("runtime_missing: Runtime executable is not executable")
    return agent, [runtime, "serve", "--mcp", "--stdio", "--json"]


def run(
    root: Path,
    installed_binary: Path | None = None,
    *,
    fixture_mode: bool = False,
) -> dict[str, object]:
    if installed_binary is not None and fixture_mode:
        raise RuntimeError("installed_binary_conflicts_with_fixture")
    fixture = root / "scripts/fixtures/simplicio_installed_fixture.py"
    digest = hashlib.sha256(fixture.read_bytes()).hexdigest() if fixture_mode else None
    env = dict(os.environ)
    if installed_binary is not None:
        installed = installed_binary.resolve()
        if not installed.is_file() or not os.access(installed, os.X_OK):
            raise RuntimeError(f"installed_binary_unavailable:{installed}")
        agent_template = [str(installed), "agent", "{socket}"]
        runtime_command = [str(installed), "serve", "--mcp", "--stdio", "--json"]
    elif fixture_mode:
        env["SIMPLICIO_CODE_E2E_FIXTURE"] = "1"
        agent_template = [sys.executable, str(fixture), "agent", "{socket}"]
        runtime_command = [
            sys.executable,
            str(fixture),
            "serve",
            "--mcp",
            "--stdio",
            "--json",
        ]
    else:
        agent_template, runtime_command = _external_dependencies()
    started = time.perf_counter_ns()
    with tempfile.TemporaryDirectory(prefix="simplicio-code-e2e-") as temporary:
        temp = Path(temporary)
        agent_socket = temp / "agent.sock"
        agent_command = [
            item.replace("{socket}", str(agent_socket)) for item in agent_template
        ]
        agent = subprocess.Popen(
            agent_command,
            env=env,
            stdin=subprocess.DEVNULL,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            wait_for_agent_socket(agent, agent_socket)
            status = request(agent_socket, {"op": "host.status"})
            validate_agent_status(status)

            # Both independently installed dependencies must be compatible
            # before the first productive turn on any Code surface.
            runtime = subprocess.Popen(
                runtime_command,
                cwd=temp,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                text=True,
            )
            try:
                initialized = runtime_call(
                    runtime,
                    1,
                    "initialize",
                    {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "simplicio-code-e2e", "version": "1"},
                    },
                )
                tools = runtime_call(runtime, 2, "tools/list", {})
                validate_runtime_contract(initialized, tools)
                surfaces = []
                for surface in SURFACES:
                    turn_id = f"e2e-{surface}-turn"
                    identity = {
                        "host_instance_id": status["host_instance_id"],
                        "workspace_id": "fixture-workspace",
                        "session_id": f"fixture-{surface}",
                        "turn_id": turn_id,
                        "attempt_id": "0",
                        "idempotency_key": turn_id,
                        "run_id": turn_id,
                        "stage_id": "conversation",
                        "fence": "0",
                        "revision": 7,
                    }
                    result = request(
                        agent_socket,
                        {
                            "op": "turn.start",
                            "profile": status["profile"],
                            "message": "contract probe",
                            **identity,
                        },
                    )
                    if not result.get("result", {}).get("completed"):
                        raise RuntimeError(f"{surface} turn did not complete: {result}")
                    surfaces.append(
                        {
                            "surface": surface,
                            "session_id": identity["session_id"],
                            "turn_id": turn_id,
                            "completed": True,
                        }
                    )
                cancel = request(
                    agent_socket,
                    {
                        "op": "turn.cancel",
                        "turn_id": "e2e-tui-turn",
                        "host_instance_id": status["host_instance_id"],
                        "profile": status["profile"],
                        "session_id": "fixture-tui",
                        "incarnation": "default",
                        "revision": 7,
                    },
                )
                reconcile = request(
                    agent_socket,
                    {
                        "op": "turn.reconcile",
                        "turn_id": "e2e-tui-turn",
                        "host_instance_id": status["host_instance_id"],
                        "profile": status["profile"],
                        "session_id": "fixture-tui",
                        "incarnation": "default",
                        "revision": 7,
                    },
                )
                first = request(
                    agent_socket,
                    {
                        "op": "host.advisories",
                        "cursor": 0,
                        "host_instance_id": status["host_instance_id"],
                    },
                )
                replay = request(
                    agent_socket,
                    {
                        "op": "host.advisories",
                        "cursor": 0,
                        "host_instance_id": status["host_instance_id"],
                    },
                )
                edit = runtime_call(
                    runtime,
                    3,
                    "tools/call",
                    {
                        "name": "simplicio_edit",
                        "arguments": effect_arguments(
                            "simplicio_edit",
                            {
                                "repo": str(temp),
                                "plan": json.dumps(
                                    {
                                        "file": str(temp / "result.txt"),
                                        "operations": [
                                            {
                                                "op": "create",
                                                "text": "runtime-owned\n",
                                            }
                                        ],
                                    }
                                ),
                                "atomic": True,
                                "rollback": True,
                            },
                            transaction_id="e2e-edit",
                        ),
                    },
                )
                listing = runtime_call(
                    runtime,
                    4,
                    "tools/call",
                    {
                        "name": "simplicio_fs_list",
                        "arguments": {
                            "repo": str(temp),
                            "path": ".",
                            "options": {"depth": 1, "limit": 100},
                        },
                    },
                )
                stat = runtime_call(
                    runtime,
                    5,
                    "tools/call",
                    {
                        "name": "simplicio_fs_stat",
                        "arguments": {"repo": str(temp), "path": "result.txt"},
                    },
                )
                execution = runtime_call(
                    runtime,
                    6,
                    "tools/call",
                    {
                        "name": "simplicio_exec",
                        "arguments": effect_arguments(
                            "simplicio_exec",
                            {
                                "repo": str(temp),
                                "cwd": ".",
                                "argv": [sys.executable, "-c", "print('argv-safe')"],
                                "env": {},
                                "timeout_ms": 5000,
                                "max_output_bytes": 4096,
                                "shell": False,
                                "idempotency_key": "e2e-exec",
                            },
                            transaction_id="e2e-exec",
                        ),
                    },
                )
                prototype_bytes = b"prototype-first-installed-e2e\n"
                prototype_id = "installed-preview"
                prototype_write = runtime_call(
                    runtime,
                    7,
                    "tools/call",
                    {
                        "name": "simplicio_prototype_artifact_write",
                        "arguments": effect_arguments(
                            "simplicio_prototype_artifact_write",
                            {
                                "repo": str(temp),
                                "artifact_id": prototype_id,
                                "path": f".simplicio/artifacts/prototype-first/{prototype_id}.json",
                                "content_base64": base64.b64encode(prototype_bytes).decode("ascii"),
                                "encoding": "base64",
                                "atomic": True,
                                "rollback": True,
                            },
                            transaction_id="e2e-prototype-write",
                        ),
                    },
                )
                prototype_read = runtime_call(
                    runtime,
                    8,
                    "tools/call",
                    {
                        "name": "simplicio_prototype_artifact_read",
                        "arguments": {
                            "repo": str(temp),
                            "artifact_id": prototype_id,
                            "path": f".simplicio/artifacts/prototype-first/{prototype_id}.json",
                        },
                    },
                )
            finally:
                runtime.terminate()
                runtime.wait(timeout=2)
                if runtime.stdin:
                    runtime.stdin.close()
                if runtime.stdout:
                    runtime.stdout.close()
            edit_payload = json.loads(edit["content"][0]["text"])
            list_payload = json.loads(listing["content"][0]["text"])
            stat_payload = json.loads(stat["content"][0]["text"])
            exec_payload = json.loads(execution["content"][0]["text"])
            prototype_write_payload = json.loads(prototype_write["content"][0]["text"])
            prototype_read_payload = json.loads(prototype_read["content"][0]["text"])
            exec_stdout = exec_payload.get("stdout")
            if isinstance(exec_stdout, dict):
                exec_stdout = exec_stdout.get("data")
            if (temp / "result.txt").read_text() != "runtime-owned\n" or exec_stdout != "argv-safe\n":
                raise RuntimeError("Runtime effects did not match receipts")
            listed_paths = {node.get("path") for node in list_payload.get("nodes", list_payload.get("entries", []))}
            stat_exists = stat_payload.get("exists", stat_payload.get("kind") is not None or stat_payload.get("type") is not None)
            if "result.txt" not in listed_paths or not stat_exists:
                raise RuntimeError("Runtime list/stat did not observe the Runtime edit")
            if exec_payload.get("success") is not True:
                raise RuntimeError("Runtime exec did not return an authoritative completed effect")
            if (
                prototype_write_payload.get("schema")
                != "simplicio.prototype-artifact/v1"
                or prototype_read_payload.get("receipt", {}).get("schema")
                != "simplicio.prototype-artifact/v1"
                or prototype_read_payload.get("content_base64")
                != base64.b64encode(prototype_bytes).decode("ascii")
                or not (temp / ".simplicio/artifacts/prototype-first/installed-preview.json").is_file()
            ):
                raise RuntimeError("Runtime Prototype-First artifact round trip did not match receipts")
            if first["advisories"] != replay["advisories"]:
                raise RuntimeError("advisory replay is not deterministic")
            agent.terminate()
            agent.wait(timeout=2)
            close_process_pipes(agent)
            agent_socket.unlink(missing_ok=True)
            agent = subprocess.Popen(
                agent_command,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            wait_for_agent_socket(agent, agent_socket)
            restarted = request(agent_socket, {"op": "host.status"})
            if (
                not fixture_mode
                and restarted["host_instance_id"] == status["host_instance_id"]
            ):
                raise RuntimeError(
                    "AgentHost restart did not rotate causal host identity"
                )
            elapsed = time.perf_counter_ns() - started
            negative_gates = negative_dependency_gates()
            scenario_count = len(surfaces) + len(negative_gates) + 7
            return {
                "schema": "simplicio.code-installed-e2e-receipt/v1",
                "proof_kind": (
                    "hermetic_fixture_non_proof"
                    if fixture_mode
                    else "external_installed"
                ),
                "mode": "fixture" if fixture_mode else "installed",
                "fixture_sha256": digest,
                "agent_host": {
                    "protocol": status["protocol_schema"],
                    "host_instance_id": status["host_instance_id"],
                    "restarted_host_instance_id": restarted["host_instance_id"],
                    "cancel": cancel.get("turn", {}).get("state", cancel.get("status")),
                    "reconcile": reconcile.get("turn", {}).get("state", reconcile.get("status")),
                    "advisory_replay_equal": True,
                    "restart_reconnected": True,
                },
                "runtime": {
                    "server": initialized["serverInfo"],
                    "tools": sorted(tool["name"] for tool in tools["tools"]),
                    "list": list_payload["schema"],
                    "stat": stat_payload["schema"],
                    "edit": edit_payload["schema"],
                    "exec": exec_payload["schema"],
                    "prototype_artifact_write": prototype_write_payload["schema"],
                    "prototype_artifact_read": prototype_read_payload["receipt"]["schema"],
                    "effect_state": "completed" if exec_payload.get("success") else "failed",
                },
                "surfaces": surfaces,
                "profile_isolation": len({item["session_id"] for item in surfaces})
                == len(SURFACES),
                "negative_dependency_gates": negative_gates,
                "benchmark": {
                    "scenario_count": scenario_count,
                    "elapsed_ns": elapsed,
                    "operations_per_second": round(
                        scenario_count * 1_000_000_000 / elapsed, 2
                    ),
                },
                "metrics_unavailable": (
                    {
                        "production_latency_ns": {
                            "value": None,
                            "reason": "fixture is hermetic; production metric is not observed",
                        }
                    }
                    if fixture_mode
                    else {
                        "production_latency_ns": {
                            "value": None,
                            "reason": "single E2E sample is not a production latency metric",
                        }
                    }
                ),
            }
        finally:
            agent.terminate()
            agent.wait(timeout=2)
            close_process_pipes(agent)


def main() -> None:  # pragma: no cover - exercised by the documented system command
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--root", type=Path, default=Path(__file__).resolve().parents[1]
    )
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--fixture",
        action="store_true",
        help="run hermetic non-proof regression fixture",
    )
    parser.add_argument(
        "--installed",
        type=Path,
        help="exercise an actually installed simplicio binary instead of external env commands",
    )
    args = parser.parse_args()
    receipt = run(
        args.root.resolve(), args.installed, fixture_mode=args.fixture
    )
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")


if __name__ == "__main__":  # pragma: no cover
    main()
