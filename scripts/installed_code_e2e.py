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
        "simplicio_map",
        "simplicio_prototype_artifact_read",
        "simplicio_prototype_artifact_write",
        "simplicio_test_run",
    )
)
AGENT_STARTUP_TIMEOUT_S = 30.0
REQUIRED_AGENT_CAPABILITIES = frozenset(
    ("host.advisories", "host.status", "turn.cancel", "turn.reconcile", "turn.start")
)
FAST_MODES = ("rust", "python", "off")
FAST_MATRIX_SCHEMA = "simplicio.code-fast-mode-matrix/v1"
FAST_PROBE_TIMEOUT_S = 10.0
RUNTIME_RELEASE_SCHEMA = "simplicio.release-manifest/v1"
REQUIRED_RUNTIME_PROCESS_CAPABILITIES = frozenset((
    "start", "status", "cancel", "wait"
))


def _validate_fast_modes(modes: tuple[str, ...]) -> tuple[str, ...]:
    normalized = tuple(str(mode).strip().lower() for mode in modes)
    if not normalized or any(not mode for mode in normalized):
        raise ValueError("fast_modes_empty")
    unknown = sorted(set(normalized) - set(FAST_MODES))
    if unknown:
        raise ValueError("fast_mode_unknown:" + ",".join(unknown))
    if len(set(normalized)) != len(normalized):
        raise ValueError("fast_mode_duplicate")
    return normalized


def parse_fast_modes(value: str | None) -> tuple[str, ...]:
    return _validate_fast_modes(
        FAST_MODES if value is None else tuple(value.split(","))
    )


def _json_payload(output: str) -> dict[str, object]:
    try:
        payload = json.loads(output)
    except json.JSONDecodeError:
        payload = None
    if isinstance(payload, dict):
        return payload
    for line in reversed(output.splitlines()):
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            continue
        if isinstance(payload, dict):
            return payload
    return {}


def _fast_mode_result(
    mode: str,
    *,
    executable: str | None,
    fixture_mode: bool,
    runner: object,
) -> dict[str, object]:
    started = time.perf_counter_ns()
    result: dict[str, object] = {
        "mode": mode,
        "requested_engine": mode,
        "outcome": "blocked",
        "probe_status": "not_executed",
        "selected_engine": None,
        "version": None,
        "reason": None,
        "effect_attempted": False,
        "local_llm_started": False,
    }
    if mode == "off":
        result.update(
            outcome="not_executed",
            reason="fast_disabled_by_request",
        )
    elif fixture_mode:
        result.update(
            outcome="not_executed",
            reason="hermetic_fixture_does_not_start_fast",
        )
    elif executable is None:
        result.update(
            outcome="blocked",
            reason="fast_binary_missing",
        )
    else:
        try:
            completed = runner(
                [
                    executable,
                    "--fast-engine",
                    mode,
                    "capabilities",
                ],
                capture_output=True,
                text=True,
                timeout=FAST_PROBE_TIMEOUT_S,
                check=False,
            )
            payload = _json_payload(
                str(getattr(completed, "stdout", ""))
            )
            engine = payload.get("engine")
            engine = engine if isinstance(engine, dict) else {}
            selected = payload.get("selected_engine")
            if not isinstance(selected, str):
                selected = engine.get("selected_engine")
            version = payload.get("version")
            if not isinstance(version, str):
                manifest = engine.get("manifest")
                manifest = manifest if isinstance(manifest, dict) else {}
                version = manifest.get("version")
            result.update(
                outcome=(
                    "ready"
                    if getattr(completed, "returncode", 1) == 0
                    and selected == mode
                    else "blocked"
                ),
                probe_status=(
                    "passed"
                    if getattr(completed, "returncode", 1) == 0
                    else "failed"
                ),
                selected_engine=selected if isinstance(selected, str) else None,
                version=version if isinstance(version, str) else None,
                reason=(
                    None
                    if getattr(completed, "returncode", 1) == 0
                    and selected == mode
                    else payload.get("reason", "fast_engine_unavailable")
                ),
            )
        except (OSError, subprocess.SubprocessError, TypeError):
            result.update(
                outcome="blocked",
                probe_status="failed",
                reason="fast_probe_failed",
            )
    result["elapsed_ms"] = round(
        (time.perf_counter_ns() - started) / 1_000_000, 3
    )
    return result


def build_fast_mode_matrix(
    modes: tuple[str, ...] = FAST_MODES,
    *,
    fixture_mode: bool = False,
    executable: str | None = None,
    runner: object = subprocess.run,
) -> dict[str, object]:
    normalized = _validate_fast_modes(tuple(modes))
    if executable is None and not fixture_mode:
        executable = (
            os.environ.get("SIMPLICIO_FAST_BIN")
            or shutil.which("simplicio-fast")
        )
    return {
        "schema": FAST_MATRIX_SCHEMA,
        "requested_modes": list(normalized),
        "results": [
            _fast_mode_result(
                mode,
                executable=executable,
                fixture_mode=fixture_mode,
                runner=runner,
            )
            for mode in normalized
        ],
        "productive_flow_attempted": False,
    }



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


def missing_runtime_process_capabilities(initialized: dict[str, object]) -> tuple[str, ...]:
    capabilities = initialized.get("capabilities")
    capabilities = capabilities if isinstance(capabilities, dict) else {}
    process = capabilities.get("runtime_process")
    process = process if isinstance(process, dict) else {}
    return tuple(
        sorted(
            name
            for name in REQUIRED_RUNTIME_PROCESS_CAPABILITIES
            if process.get(name) is not True
        )
    )


def validate_runtime_process_capabilities(initialized: dict[str, object]) -> None:
    missing = missing_runtime_process_capabilities(initialized)
    if missing:
        raise RuntimeError("runtime_process_incompatible:" + ",".join(missing))


def validate_runtime_contract(
    initialized: dict[str, object] | None, tools: dict[str, object] | None
) -> None:
    """Require the Runtime handshake and effect tools before Agent turns."""
    if initialized is None or tools is None:
        raise RuntimeError("runtime_missing")
    if initialized.get("protocolVersion") != "2024-11-05":
        raise RuntimeError("runtime_incompatible")
    validate_runtime_process_capabilities(initialized)
    advertised = {
        tool.get("name") for tool in tools.get("tools", []) if isinstance(tool, dict)
    }
    if not REQUIRED_RUNTIME_TOOLS.issubset(advertised):
        raise RuntimeError("runtime_incompatible")


def build_component_manifest(
    status: dict[str, object],
    initialized: dict[str, object],
    tools: dict[str, object],
    *,
    agent_command: list[str],
    runtime_command: list[str],
    fixture_mode: bool,
) -> dict[str, object]:
    """Describe the independently observed components behind this receipt."""
    return {
        "schema": "simplicio.installed-components/v1",
        "proof_kind": (
            "hermetic_fixture_non_proof" if fixture_mode else "external_installed"
        ),
        "agent_host": {
            "protocol_schema": status["protocol_schema"],
            "agent_protocol": status["agent_protocol"],
            "capabilities": sorted(status["capabilities"]),
            "profile": status["profile"],
        },
        "runtime": {
            "protocol_version": initialized["protocolVersion"],
            "server": initialized["serverInfo"],
            "tools": sorted(tool["name"] for tool in tools["tools"]),
        },
        "surfaces": list(SURFACES),
        "components": [
            build_component_entry(
                "code",
                str(Path(__file__).resolve()),
                kind="source_harness",
                proof_kind="source_harness",
            ),
            build_component_entry(
                "agent_host",
                agent_command[0] if agent_command else None,
                kind="process_entrypoint",
                proof_kind=(
                    "hermetic_fixture_non_proof"
                    if fixture_mode
                    else "external_installed"
                ),
                version=(
                    status.get("version")
                    if isinstance(status.get("version"), str)
                    else None
                ),
            ),
            build_component_entry(
                "runtime",
                runtime_command[0] if runtime_command else None,
                kind="process_entrypoint",
                proof_kind=(
                    "hermetic_fixture_non_proof"
                    if fixture_mode
                    else "external_installed"
                ),
                version=initialized["serverInfo"].get("version"),
            ),
            build_component_entry(
                "loop_hub",
                os.environ.get("SIMPLICIO_LOOP_BIN")
                or shutil.which("simplicio-loop"),
                kind="installed_adapter",
                proof_kind="installed_observation",
            ),
            build_component_entry(
                "mapper",
                os.environ.get("SIMPLICIO_MAPPER_BIN")
                or shutil.which("simplicio-mapper"),
                kind="installed_adapter",
                proof_kind="installed_observation",
            ),
            build_component_entry(
                "dev_cli",
                os.environ.get("SIMPLICIO_DEV_CLI_BIN")
                or shutil.which("simplicio-dev-cli"),
                kind="installed_adapter",
                proof_kind="installed_observation",
            ),
            build_component_entry(
                "fast",
                os.environ.get("SIMPLICIO_FAST_BIN")
                or shutil.which("simplicio-fast"),
                kind="installed_adapter",
                proof_kind="installed_observation",
            ),
        ],
    }


def build_process_observations(
    first_agent_pid: int | None,
    agent_pid: int | None,
    runtime_pid: int | None,
    agent_command: list[str],
    runtime_command: list[str],
    *,
    fixture_mode: bool,
) -> dict[str, object]:
    return {
        "schema": "simplicio.process-observations/v1",
        "proof_kind": (
            "hermetic_fixture_non_proof" if fixture_mode else "external_installed"
        ),
        "independent": (
            agent_pid is not None
            and runtime_pid is not None
            and agent_pid != runtime_pid
        ),
        "restart": {
            "initial_agent_pid": first_agent_pid,
            "restarted_agent_pid": agent_pid,
            "rotated": (
                first_agent_pid is not None
                and agent_pid is not None
                and first_agent_pid != agent_pid
            ),
        },
        "processes": [
            {
                "role": "agent_host",
                "pid": agent_pid,
                "executable": agent_command[0] if agent_command else None,
                "transport": "loopback_tcp" if os.name == "nt" else "unix_socket",
            },
            {
                "role": "runtime",
                "pid": runtime_pid,
                "executable": runtime_command[0] if runtime_command else None,
                "transport": "stdio",
            },
        ],
    }

def build_component_entry(
    role: str,
    executable: str | None,
    *,
    kind: str,
    proof_kind: str,
    version: str | None = None,
) -> dict[str, object]:
    candidate = Path(executable).resolve() if executable else None
    digest = None
    observed_version = version
    if (
        observed_version is None
        and kind != "source_harness"
        and proof_kind != "hermetic_fixture_non_proof"
    ):
        observed_version = read_component_version(executable)
    if candidate is not None and candidate.is_file():
        try:
            digest = hashlib.sha256(candidate.read_bytes()).hexdigest()
        except OSError:
            candidate = None
    return {
        "role": role,
        "kind": kind,
        "status": "observed" if candidate is not None else "missing",
        "proof_kind": proof_kind,
        "executable": executable,
        "path": str(candidate) if candidate is not None else None,
        "version": observed_version,
        "sha256": digest,
    }


def read_component_version(executable: str | None) -> str | None:
    if not executable:
        return None
    try:
        result = subprocess.run(
            [executable, "--version", "--json"],
            capture_output=True,
            text=True,
            timeout=3,
            check=False,
        )
    except (OSError, subprocess.SubprocessError):
        return None
    for line in (result.stdout + "\n" + result.stderr).splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            payload = json.loads(line)
        except json.JSONDecodeError:
            return line[:200]
        if isinstance(payload, dict) and isinstance(payload.get("version"), str):
            return payload["version"]
        return line[:200]
    return None

def read_runtime_release_identity(
    executable: str | None,
    *,
    runner: object = subprocess.run,
) -> dict[str, object]:
    """Probe and redact the installed Runtime release identity before effects."""
    if not executable:
        raise RuntimeError("runtime_missing: executable is unavailable")
    try:
        result = runner(
            [executable, "version", "--json"],
            capture_output=True,
            text=True,
            timeout=3,
            check=False,
        )
    except (OSError, subprocess.SubprocessError, TypeError) as error:
        raise RuntimeError("runtime_incompatible: release manifest probe failed") from error
    payload = _json_payload(
        str(getattr(result, "stdout", ""))
        + "\n"
        + str(getattr(result, "stderr", ""))
    )
    runtime = payload.get("runtime")
    capabilities = payload.get("capabilities")
    if (
        getattr(result, "returncode", 1) != 0
        or payload.get("schema") != RUNTIME_RELEASE_SCHEMA
        or not isinstance(runtime, dict)
        or not isinstance(runtime.get("name"), str)
        or not runtime["name"]
        or not isinstance(runtime.get("version"), str)
        or not runtime["version"]
        or not isinstance(runtime.get("commit"), str)
        or not runtime["commit"]
        or not isinstance(runtime.get("target"), str)
        or not runtime["target"]
        or not isinstance(capabilities, list)
        or not capabilities
        or not all(isinstance(item, str) and item for item in capabilities)
    ):
        raise RuntimeError(
            "runtime_incompatible: release manifest missing version/capabilities/commit/target"
        )
    try:
        digest = hashlib.sha256(Path(executable).resolve().read_bytes()).hexdigest()
    except OSError as error:
        raise RuntimeError("runtime_incompatible: cannot hash Runtime executable") from error
    return {
        "schema": RUNTIME_RELEASE_SCHEMA,
        "name": runtime["name"],
        "version": runtime["version"],
        "commit": runtime.get("commit"),
        "target": runtime.get("target"),
        "binary": payload.get("binary"),
        "capabilities": sorted(capabilities),
        "sha256": digest,
    }


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


def read_tcp_endpoint(
    socket_path: Path,
) -> tuple[str, int, str | None] | None:
    """Read a loopback endpoint and its optional authenticated sidecar."""
    candidates: list[Path] = []
    if socket_path.is_file():
        candidates.append(socket_path)
    sidecar = socket_path.with_suffix(".tcp")
    if sidecar != socket_path and sidecar.is_file():
        candidates.append(sidecar)
    token_path = socket_path.with_suffix(".token")
    for endpoint_path in candidates:
        try:
            raw = endpoint_path.read_text(encoding="ascii").strip()
        except OSError as error:
            raise RuntimeError("agent_endpoint_unavailable") from error
        if raw.startswith("tcp://"):
            raw = raw.removeprefix("tcp://")
        if ":" not in raw:
            continue
        host, port_text = raw.rsplit(":", 1)
        if host != "127.0.0.1":
            raise RuntimeError("agent_endpoint_not_loopback")
        try:
            port = int(port_text)
        except ValueError as error:
            raise RuntimeError("agent_endpoint_invalid") from error
        if not 1 <= port <= 65535:
            raise RuntimeError("agent_endpoint_invalid")
        token = None
        if token_path.is_file():
            try:
                token = token_path.read_text(encoding="ascii").strip()
            except OSError as error:
                raise RuntimeError("agent_endpoint_auth_unavailable") from error
            if not 32 <= len(token) <= 256:
                raise RuntimeError("agent_endpoint_auth_invalid")
        elif endpoint_path == sidecar:
            raise RuntimeError("agent_endpoint_auth_missing")
        return host, port, token
    return None


def request(socket_path: Path, payload: dict[str, object]) -> dict[str, object]:
    endpoint = read_tcp_endpoint(socket_path)
    if endpoint is not None:
        host, port, token = endpoint
        request_payload = {**payload, "auth_token": token} if token else payload
        client = socket.create_connection((host, port))
    else:
        request_payload = payload
        family = getattr(socket, "AF_UNIX", None)
        if family is None:
            raise RuntimeError("agent_transport_unavailable")
        client = socket.socket(family)
        client.connect(str(socket_path))
    with client:
        client.sendall(json.dumps(request_payload).encode())
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
        if read_tcp_endpoint(socket_path) is not None:
            return
        if socket_path.exists() and getattr(socket, "AF_UNIX", None) is not None:
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


def runtime_text(response: dict[str, object]) -> str:
    content = response.get("content")
    if not isinstance(content, list) or not content or not isinstance(content[0], dict):
        raise RuntimeError("invalid Runtime content")
    text = content[0].get("text")
    if not isinstance(text, str):
        raise RuntimeError("invalid Runtime text")
    return text


def prototype_artifact_schema(payload: dict[str, object]) -> str | None:
    receipt = payload.get("receipt")
    receipt_schema = receipt.get("schema") if isinstance(receipt, dict) else None
    return (
        payload.get("artifact_schema")
        or receipt_schema
        or payload.get("schema")
    )


def prototype_artifact_bytes(payload: dict[str, object]) -> bytes | None:
    content = payload.get("content")
    if isinstance(content, str):
        return content.encode("utf-8")
    encoded = payload.get("content_base64")
    if not isinstance(encoded, str):
        return None
    try:
        return base64.b64decode(encoded, validate=True)
    except (ValueError, base64.binascii.Error):
        return None


def runtime_exec_version(payload: dict[str, object]) -> str | None:
    runtime = payload.get("runtime")
    if isinstance(runtime, dict) and isinstance(runtime.get("version"), str):
        return runtime["version"]
    return payload.get("version") if isinstance(payload.get("version"), str) else None


def _windows_wrapper_target(path: Path) -> Path | None:
    """Find the first executable target in a Windows command wrapper."""
    if path.suffix.lower() not in {".bat", ".cmd"}:
        return None
    try:
        content = path.read_text(encoding="utf-8", errors="replace")
    except OSError as error:
        raise RuntimeError(f"agent_host_missing: cannot read wrapper: {path}") from error
    for line in content.splitlines():
        stripped = line.strip()
        if not stripped or stripped.lower().startswith(("rem ", "::", "@echo", "set ")):
            continue
        parts = stripped.split()
        if len(parts) < 2:
            continue
        placeholder = parts[-1]
        if placeholder != "%*" and not (
            placeholder.startswith("%") and placeholder[1:].isdigit()
        ):
            continue
        executable = parts[0]
        if executable.startswith('"') and executable.endswith('"'):
            executable = executable[1:-1]
        return Path(executable)
    return None


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
    agent_executable = Path(shutil.which(agent[0]) or agent[0])
    if not agent_executable.is_file() or not os.access(agent_executable, os.X_OK):
        raise RuntimeError("agent_host_missing: AgentHost executable is not executable")
    wrapper_target = _windows_wrapper_target(agent_executable)
    if wrapper_target is not None:
        target = Path(shutil.which(str(wrapper_target)) or wrapper_target)
        if not target.is_file() or not os.access(target, os.X_OK):
            raise RuntimeError(
                f"agent_host_missing: wrapper target is not executable: {target}"
            )
    runtime_path = Path(runtime)
    if not runtime_path.is_file() or not os.access(runtime_path, os.X_OK):
        raise RuntimeError("runtime_missing: Runtime executable is not executable")
    return agent, [str(runtime_path), "serve", "--mcp", "--stdio", "--json"]


def diagnose_installed_dependencies() -> dict[str, object]:
    """Probe executable dependencies without starting a productive process."""
    try:
        agent_command, runtime_command = _external_dependencies()
    except RuntimeError as error:
        return {
            "schema": "simplicio.installed-dependency-diagnostic/v1",
            "proof_kind": "installed_observation",
            "status": "blocked",
            "effect_attempted": False,
            "productive_flow_verified": False,
            "reason": str(error),
        }
    return {
        "schema": "simplicio.installed-dependency-diagnostic/v1",
        "proof_kind": "installed_observation",
        "status": "preflight_ready",
        "effect_attempted": False,
        "productive_flow_verified": False,
        "agent_executable": agent_command[0],
        "runtime_executable": runtime_command[0],
    }


def run(
    root: Path,
    installed_binary: Path | None = None,
    *,
    fixture_mode: bool = False,
    fast_modes: tuple[str, ...] = FAST_MODES,
) -> dict[str, object]:
    fast_modes = _validate_fast_modes(tuple(fast_modes))
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
    runtime_release_identity = None
    if not fixture_mode:
        runtime_release_identity = read_runtime_release_identity(runtime_command[0])

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
            first_agent_pid = agent.pid
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
                stderr=subprocess.PIPE,
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
                project_map = runtime_call(
                    runtime,
                    3,
                    "tools/call",
                    {
                        "name": "simplicio_map",
                        "arguments": {"repo": str(temp)},
                    },
                )
                edit = runtime_call(
                    runtime,
                    4,
                    "tools/call",
                    {
                        "name": "simplicio_edit",
                        "arguments": effect_arguments(
                            "simplicio_edit",
                            {
                                "repo": str(temp),
                                "plan": json.dumps(
                                    {
                                        "file": "result.txt",
                                        "operations": [
                                            {
                                                "op": "create",
                                                "text": "runtime-owned\n",
                                            }
                                        ],
                                    }
                                ),
                            },
                            transaction_id="e2e-edit",
                        ),
                    },
                )
                readback = runtime_call(
                    runtime,
                    5,
                    "tools/call",
                    {
                        "name": "simplicio_file_read",
                        "arguments": {
                            "repo": str(temp),
                            "path": "result.txt",
                        },
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
                            {"repo": str(temp), "command": "version --json"},
                            transaction_id="e2e-exec",
                        ),
                    },
                )
                test_run = runtime_call(
                    runtime,
                    7,
                    "tools/call",
                    {
                        "name": "simplicio_test_run",
                        "arguments": effect_arguments(
                            "simplicio_test_run",
                            {
                                "repo": str(temp),
                                "cmd": sys.executable,
                                "args": ["-c", "print('argv-safe')"],
                            },
                            transaction_id="e2e-test-run",
                        ),
                    },
                )
                prototype_text = "prototype-first-installed-e2e\n"
                prototype_bytes = prototype_text.encode("utf-8")
                prototype_id = "installed-preview"
                prototype_write = runtime_call(
                    runtime,
                    8,
                    "tools/call",
                    {
                        "name": "simplicio_prototype_artifact_write",
                        "arguments": effect_arguments(
                            "simplicio_prototype_artifact_write",
                            {
                                "repo": str(temp),
                                "artifact_id": prototype_id,
                                "content": prototype_text,
                            },
                            transaction_id="e2e-prototype-write",
                        ),
                    },
                )
                prototype_write_retry = runtime_call(
                    runtime,
                    9,
                    "tools/call",
                    {
                        "name": "simplicio_prototype_artifact_write",
                        "arguments": effect_arguments(
                            "simplicio_prototype_artifact_write",
                            {
                                "repo": str(temp),
                                "artifact_id": prototype_id,
                                "content": prototype_text,
                                "overwrite": True,
                            },
                            transaction_id="e2e-prototype-write",
                        ),
                    },
                )
                prototype_read = runtime_call(
                    runtime,
                    10,
                    "tools/call",
                    {
                        "name": "simplicio_prototype_artifact_read",
                        "arguments": {
                            "repo": str(temp),
                            "artifact_id": prototype_id,
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
            runtime_restart = subprocess.Popen(
                runtime_command,
                cwd=temp,
                env=env,
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            runtime_restart_pid = runtime_restart.pid
            try:
                restarted_initialized = runtime_call(
                    runtime_restart,
                    11,
                    "initialize",
                    {
                        "protocolVersion": "2024-11-05",
                        "capabilities": {},
                        "clientInfo": {"name": "simplicio-code-e2e-restart", "version": "1"},
                    },
                )
                restarted_tools = runtime_call(runtime_restart, 12, "tools/list", {})
                validate_runtime_contract(restarted_initialized, restarted_tools)
            finally:
                runtime_restart.terminate()
                runtime_restart.wait(timeout=2)
                close_process_pipes(runtime_restart)
            restarted_tool_names = sorted(
                tool["name"] for tool in restarted_tools["tools"]
            )
            runtime_restart_match = (
                restarted_initialized.get("serverInfo") == initialized.get("serverInfo")
                and restarted_tool_names == sorted(
                    tool["name"] for tool in tools["tools"]
                )
            )
            if not runtime_restart_match:
                raise RuntimeError("Runtime restart contract did not match initial handshake")

            map_text = runtime_text(project_map)
            edit_payload = json.loads(runtime_text(edit))
            readback_payload = json.loads(runtime_text(readback))
            exec_payload = json.loads(runtime_text(execution))
            test_payload = json.loads(runtime_text(test_run))
            prototype_write_payload = json.loads(runtime_text(prototype_write))
            prototype_write_retry_payload = json.loads(runtime_text(prototype_write_retry))
            prototype_read_payload = json.loads(runtime_text(prototype_read))
            exec_version = runtime_exec_version(exec_payload)
            expected_runtime_version = initialized["serverInfo"].get("version")
            test_output = test_payload.get("output_tail", "")
            exec_completed = (
                exec_payload.get("success") is True
                or exec_version == expected_runtime_version
            )
            if (
                not map_text.strip()
                or (temp / "result.txt").read_text() != "runtime-owned\n"
                or "argv-safe" not in str(test_output)
            ):
                raise RuntimeError("Runtime map/edit/test effects did not match receipts")
            if readback_payload.get("schema") != "simplicio.read-result/v1":
                raise RuntimeError("Runtime file_read did not return an authoritative receipt")
            if not exec_completed or test_payload.get("exit_code") != 0:
                raise RuntimeError("Runtime execution did not return an authoritative completed effect")
            prototype_read_bytes = prototype_artifact_bytes(prototype_read_payload)
            artifact_root = temp / ".simplicio" / "artifacts" / "prototype-first"
            artifact_paths = (artifact_root / prototype_id, artifact_root / f"{prototype_id}.json")
            if (
                prototype_artifact_schema(prototype_write_payload)
                != "simplicio.prototype-artifact/v1"
                or prototype_artifact_schema(prototype_read_payload)
                != "simplicio.prototype-artifact/v1"
                or prototype_read_bytes != prototype_bytes
                or prototype_artifact_schema(prototype_write_retry_payload)
                != "simplicio.prototype-artifact/v1"
                or not any(path.is_file() for path in artifact_paths)
            ):
                raise RuntimeError("Runtime Prototype-First artifact round trip did not match receipts")
            if first["advisories"] != replay["advisories"]:
                raise RuntimeError("advisory replay is not deterministic")
            agent.terminate()
            agent.wait(timeout=2)
            close_process_pipes(agent)
            agent_socket.unlink(missing_ok=True)
            restarted_socket = temp / "agent-restarted.sock"
            restarted_command = [
                item.replace(str(agent_socket), str(restarted_socket))
                for item in agent_command
            ]
            agent = subprocess.Popen(
                restarted_command,
                env=env,
                stdin=subprocess.DEVNULL,
                stdout=subprocess.PIPE,
                stderr=subprocess.PIPE,
                text=True,
            )
            wait_for_agent_socket(agent, restarted_socket)
            restarted = request(restarted_socket, {"op": "host.status"})
            if (
                not fixture_mode
                and restarted["host_instance_id"] == status["host_instance_id"]
            ):
                raise RuntimeError(
                    "AgentHost restart did not rotate causal host identity"
                )
            elapsed = time.perf_counter_ns() - started
            negative_gates = negative_dependency_gates()
            scenario_count = len(surfaces) + len(negative_gates) + 9
            fast_matrix = build_fast_mode_matrix(
                fast_modes,
                fixture_mode=fixture_mode,
            )
            return {
                "schema": "simplicio.code-installed-e2e-receipt/v1",
                "proof_kind": (
                    "hermetic_fixture_non_proof"
                    if fixture_mode
                    else "external_installed"
                ),
                "mode": "fixture" if fixture_mode else "installed",
                "fixture_sha256": digest,
                "component_manifest": {
                    **build_component_manifest(
                        status,
                        initialized,
                        tools,
                        agent_command=agent_command,
                        runtime_command=runtime_command,
                        fixture_mode=fixture_mode,
                    ),
                    "process_observations": build_process_observations(
                        first_agent_pid,
                        agent.pid,
                        runtime.pid,
                        agent_command,
                        runtime_command,
                        fixture_mode=fixture_mode,
                    ),
                },
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
                    "release_identity": runtime_release_identity,
                    "server": initialized["serverInfo"],
                    "tools": sorted(tool["name"] for tool in tools["tools"]),
                    "restart": {
                        "reconnected": runtime_restart_match,
                        "pid": runtime_restart_pid,
                        "server": restarted_initialized["serverInfo"],
                    },
                    "restart_tools_match": runtime_restart_match,
                    "map": "simplicio.map-result/v1",
                    "map_bytes": len(map_text),
                    "read": readback_payload["schema"],
                    "edit": edit_payload["schema"],
                    "exec": exec_payload.get("schema", "simplicio.exec-result/v1"),
                    "exec_version": exec_version,
                    "test_run": test_payload["schema"],
                    "prototype_artifact_write": prototype_artifact_schema(prototype_write_payload),
                    "prototype_artifact_read": prototype_artifact_schema(prototype_read_payload),
                    "prototype_artifact_idempotent_retry": True,
                    "effect_state": "completed" if exec_completed and test_payload.get("exit_code") == 0 else "failed",
                },
                "surfaces": surfaces,
                "fast_matrix": fast_matrix,
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
    parser.add_argument(
        "--fast-modes",
        default=",".join(FAST_MODES),
        help="comma-separated Fast modes to probe: rust,python,off",
    )
    parser.add_argument(
        "--diagnose-installed",
        action="store_true",
        help="probe installed executables without starting productive processes",
    )
    args = parser.parse_args()
    if args.diagnose_installed:
        receipt = diagnose_installed_dependencies()
    else:
        receipt = run(
            args.root.resolve(),
            args.installed,
            fixture_mode=args.fixture,
            fast_modes=parse_fast_modes(args.fast_modes),
        )
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")


if __name__ == "__main__":  # pragma: no cover
    main()
