#!/usr/bin/env python3
"""Hermetic installed AgentHost/Runtime fixture for Code's productive E2E.

This executable deliberately refuses to run unless the E2E runner supplies its
private opt-in variable.  It is a contract fixture, never a product fallback.
"""

from __future__ import annotations

import json
import base64
import os
from pathlib import Path
import socket
import subprocess
import sys

HOST_ID = "code-e2e-agent-host-00000001"
TOOLS = ["simplicio_edit", "simplicio_exec", "simplicio_file_read", "simplicio_fs_delete", "simplicio_fs_list", "simplicio_fs_stat", "simplicio_fs_write", "simplicio_search", "simplicio_prototype_artifact_read", "simplicio_prototype_artifact_write"]


def _host_envelope(**extra: object) -> dict[str, object]:
    return {"ok": True, "host_instance_id": HOST_ID, "protocol_schema": "simplicio.agent-host/v1", "protocol_version": 1, "agent_protocol": "agent/v1", "profile": "e2e", "capabilities": ["host.advisories", "host.status", "turn.cancel", "turn.reconcile", "turn.start"], "advisory_schema": "simplicio.agent-advisory/v1", **extra}


def agent_response(request: dict[str, object], state: dict[str, object]) -> dict[str, object]:
    op = request.get("op")
    if op == "host.status":
        return _host_envelope(host={"ready": True, "stopping": False, "host_instance_id": HOST_ID})
    if op == "turn.start":
        identity = {key: request.get(key) for key in ("workspace_id", "session_id", "turn_id", "attempt_id", "idempotency_key", "run_id", "stage_id", "fence", "revision")}
        if any(value is None or value == "" for value in identity.values()) or identity["turn_id"] != identity["idempotency_key"]:
            return {**_host_envelope(), "ok": False, "error": "invalid causal identity"}
        turn_id = str(identity["turn_id"])
        state.setdefault("turns", {})[turn_id] = identity
        state["sequence"] = int(state.get("sequence", 1)) + 1
        return _host_envelope(result={"final_response": f"fixture:{request.get('profile')}", "messages": [], "api_calls": 0, "completed": True, "failed": False, "interrupted": False})
    if op == "turn.cancel":
        turn_id = str(request.get("turn_id", ""))
        status = "cancelled" if turn_id in state.setdefault("turns", {}) else "not_found"
        return _host_envelope(status=status)
    if op == "turn.reconcile":
        found = str(request.get("turn_id", "")) in state.setdefault("turns", {})
        return _host_envelope(status="terminal" if found else "not_found")
    if op == "host.advisories":
        cursor = int(request.get("cursor", 0))
        events = [{"schema": "simplicio.agent-advisory/v1", "sequence": 1, "kind": "host.ready", "severity": "info", "summary": "Agent host is ready.", "action": None, "ts_wall_ns": 0}] if cursor < 1 else []
        return _host_envelope(advisories={"schema": "simplicio.agent-advisory/v1", "host_instance_id": HOST_ID, "events": events, "next_cursor": 1, "truncated": False})
    return {**_host_envelope(), "ok": False, "error": "unsupported operation"}


def serve_agent(socket_path: Path) -> None:  # pragma: no cover - system subprocess boundary
    socket_path.parent.mkdir(parents=True, exist_ok=True)
    socket_path.unlink(missing_ok=True)
    state: dict[str, object] = {}
    family = getattr(socket, "AF_UNIX", None)
    if family is not None:
        server = socket.socket(family)
        server.bind(str(socket_path))
        os.chmod(socket_path, 0o600)
    else:
        server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        server.bind(("127.0.0.1", 0))
        host, port = server.getsockname()
        socket_path.write_text(f"tcp://{host}:{port}\n", encoding="ascii")
    with server:
        server.listen(8)
        while True:
            connection, _ = server.accept()
            with connection:
                chunks = []
                while chunk := connection.recv(65536):
                    chunks.append(chunk)
                request = json.loads(b"".join(chunks))
                connection.sendall(json.dumps(agent_response(request, state), sort_keys=True).encode())


def _safe_path(repo: Path, relative: str) -> Path:
    resolved = (repo / relative).resolve()
    if resolved != repo and repo not in resolved.parents:
        raise ValueError("path escapes repository")
    return resolved


def runtime_tool(name: str, arguments: dict[str, object]) -> dict[str, object]:
    repo = Path(str(arguments.get("repo", "."))).resolve()
    if name == "simplicio_edit":
        plan = json.loads(str(arguments["plan"]))
        if isinstance(plan.get("file"), str):
            target = _safe_path(repo, plan["file"])
            target.parent.mkdir(parents=True, exist_ok=True)
            current = target.read_text(encoding="utf-8") if target.exists() else ""
            for operation in plan.get("operations", []):
                if operation.get("op") == "create":
                    current = str(operation.get("text", ""))
                elif operation.get("op") == "append":
                    current += str(operation.get("text", ""))
                elif operation.get("op") == "replace":
                    current = current.replace(
                        str(operation.get("find", "")),
                        str(operation.get("with", "")),
                    )
                else:
                    raise ValueError("unsupported edit operation")
            target.write_text(current, encoding="utf-8")
        else:
            for item in plan.get("files", []):
                target = _safe_path(repo, item["file"])
                target.parent.mkdir(parents=True, exist_ok=True)
                target.write_text(item.get("content", ""), encoding="utf-8")
        payload = {"schema": "simplicio.edit-result/v1", "accepted": True, "plan": plan, "rolled_back": False}
    elif name == "simplicio_fs_list":
        target = _safe_path(repo, str(arguments.get("path", ".")))
        payload = {"schema": "simplicio.fs-list-result/v1", "nodes": [{"name": child.name, "path": child.relative_to(repo).as_posix(), "type": "directory" if child.is_dir() else "file"} for child in sorted(target.iterdir())], "truncated": False}
    elif name == "simplicio_fs_stat":
        target = _safe_path(repo, str(arguments.get("path", ".")))
        payload = {"schema": "simplicio.fs-stat-result/v1", "exists": target.exists(), "type": "directory" if target.is_dir() else "file" if target.is_file() else None, "size": target.stat().st_size if target.exists() else None}
    elif name == "simplicio_exec":
        cwd = _safe_path(repo, str(arguments.get("cwd", ".")))
        completed = subprocess.run(arguments["argv"], cwd=cwd, env={**os.environ, **arguments.get("env", {})}, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=int(arguments.get("timeout_ms", 120000)) / 1000, check=False)
        payload = {"schema": "simplicio.exec-result/v1", "success": completed.returncode == 0, "stdout": completed.stdout, "stderr": completed.stderr, "exit_code": completed.returncode, "timed_out": False, "truncated": False, "effect_state": "completed"}
    elif name == "simplicio_prototype_artifact_write":
        artifact_id = str(arguments["artifact_id"])
        if not artifact_id or any(char not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-" for char in artifact_id):
            raise ValueError("unsafe prototype artifact id")
        content = base64.b64decode(str(arguments["content_base64"]), validate=True)
        target = _safe_path(repo, f".simplicio/artifacts/prototype-first/{artifact_id}.json")
        target.parent.mkdir(parents=True, exist_ok=True)
        if target.exists() and target.read_bytes() != content:
            raise ValueError("prototype artifact id is already bound to different content")
        created = not target.exists()
        if created:
            target.write_bytes(content)
        payload = {"schema": "simplicio.prototype-artifact/v1", "operation": "write", "artifact_id": artifact_id, "path": str(target), "bytes": len(content), "encoding": "base64", "created": created}
    elif name == "simplicio_prototype_artifact_read":
        artifact_id = str(arguments["artifact_id"])
        if not artifact_id or any(char not in "abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789._-" for char in artifact_id):
            raise ValueError("unsafe prototype artifact id")
        target = _safe_path(repo, f".simplicio/artifacts/prototype-first/{artifact_id}.json")
        content = target.read_bytes()
        payload = {"receipt": {"schema": "simplicio.prototype-artifact/v1", "operation": "read", "artifact_id": artifact_id, "path": str(target), "bytes": len(content), "encoding": "base64", "created": False}, "content_base64": base64.b64encode(content).decode("ascii")}
    else:
        payload = {"schema": "simplicio.fixture-result/v1", "accepted": True}
    return {"isError": False, "content": [{"type": "text", "text": json.dumps(payload, sort_keys=True)}]}


def serve_runtime() -> None:  # pragma: no cover - system subprocess boundary
    for line in sys.stdin:
        message = json.loads(line)
        method, request_id = message.get("method"), message.get("id")
        if request_id is None:
            continue
        if method == "initialize":
            result = {"protocolVersion": "2024-11-05", "serverInfo": {"name": "simplicio", "version": "code-e2e-fixture/1"}}
        elif method == "tools/list":
            result = {"tools": [{"name": tool} for tool in TOOLS]}
        elif method == "tools/call":
            params = message["params"]
            result = runtime_tool(params["name"], params.get("arguments", {}))
        else:
            result = {}
        print(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}, sort_keys=True), flush=True)


def main() -> None:  # pragma: no cover - system subprocess boundary
    if os.environ.get("SIMPLICIO_CODE_E2E_FIXTURE") != "1":
        raise SystemExit("refusing productive use: set by scripts/installed_code_e2e.py only")
    if len(sys.argv) == 3 and sys.argv[1] == "agent":
        serve_agent(Path(sys.argv[2]))
    elif sys.argv[1:5] == ["serve", "--mcp", "--stdio", "--json"]:
        serve_runtime()
    else:
        raise SystemExit("usage: fixture agent SOCKET | fixture serve --mcp --stdio --json")


if __name__ == "__main__":  # pragma: no cover
    main()
