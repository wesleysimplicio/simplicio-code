#!/usr/bin/env python3
"""Real-process Code worker-protocol E2E against an external Loop Hub daemon.

This harness exercises the newline JSON boundary over a real AF_UNIX socket and
keeps the remote-delivery assertion fail-closed: a Hub-generated placeholder is
not treated as a remote PR confirmation.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import time
from typing import Any, Dict


SCHEMA = "simplicio.loop-hub-client/v1"


def request(stream: socket.socket, reader, request_id: int, method: str, payload: Dict[str, Any]) -> Dict[str, Any]:
    stream.sendall((json.dumps({"schema": SCHEMA, "id": request_id, "method": method, "payload": payload}) + "\n").encode())
    line = reader.readline()
    if not line:
        raise RuntimeError(f"Hub closed the worker transport during {method}")
    value = json.loads(line)
    if value.get("schema") != SCHEMA or value.get("id") != request_id:
        raise RuntimeError(f"invalid Hub response envelope for {method}: {value}")
    return value


def launch(loop_root: Path, lock: Path, endpoint: Path) -> subprocess.Popen[str]:
    env = os.environ.copy()
    prior_pythonpath = env.get("PYTHONPATH")
    env["PYTHONPATH"] = str(loop_root) + (os.pathsep + prior_pythonpath if prior_pythonpath else "")
    return subprocess.Popen(
        [sys.executable, "-c",
         "from simplicio_loop.hub_daemon import main; raise SystemExit(main())", "serve",
         "--lock", str(lock), "--endpoint", str(endpoint), "--transport", "unix"],
        cwd=loop_root,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
    )


def wait_for_socket(process: subprocess.Popen[str], endpoint: Path) -> None:
    deadline = time.monotonic() + 10
    while time.monotonic() < deadline:
        if endpoint.exists():
            try:
                with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
                    probe.settimeout(0.2)
                    probe.connect(str(endpoint))
                return
            except OSError:
                pass
        if process.poll() is not None:
            stderr = process.stderr.read() if process.stderr else ""
            raise RuntimeError(f"Loop Hub exited before ready: {stderr[-2000:]}")
        time.sleep(0.02)
    raise TimeoutError("timed out waiting for external Loop Hub socket")


def stop(process: subprocess.Popen[str]) -> None:
    if process.poll() is not None:
        return
    process.send_signal(signal.SIGTERM)
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--loop-root", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    started_at = time.perf_counter()
    loop_root = Path(args.loop_root).resolve()
    result: Dict[str, Any] = {
        "schema": "simplicio.code-worker-e2e/v1",
        "proof_kind": "external_loop_hub_process",
        "loop_root": str(loop_root),
        "protocol": "simplicio.loop-worker/v1",
        "local_llm_started": False,
        "deepseek_started": False,
    }
    first = second = None
    try:
        with tempfile.TemporaryDirectory(prefix="simplicio-code-worker-e2e-") as directory:
            root = Path(directory)
            lock = root / "hub.lock"
            endpoint = root / "hub.sock"
            payload = {
                "schema": "simplicio.code-worker-adapter/v1",
                "protocol": "simplicio.loop-worker/v1",
                "identity": {
                    "coordinator_id": "agent-host-e2e", "session_id": "session-e2e",
                    "turn_id": "turn-e2e", "run_id": "run-e2e", "goal_id": "goal-e2e",
                },
                "idempotency_key": "code-worker-e2e-v1",
                "max_concurrency": 2,
                "tasks": [
                    {"task_id": "implement", "role": "implementer", "depends_on": [],
                     "task_contract": "perform workspace effects only through Runtime"},
                    {"task_id": "review", "role": "reviewer", "depends_on": ["implement"],
                     "task_contract": "independently review the external change"},
                ],
            }
            first = launch(loop_root, lock, endpoint)
            wait_for_socket(first, endpoint)
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(5)
                stream.connect(str(endpoint))
                reader = stream.makefile("rb")
                delegate = request(stream, reader, 1, "worker_delegate", payload)
                if not delegate.get("ok"):
                    raise RuntimeError(delegate)
                replay = request(stream, reader, 2, "worker_delegate", payload)
                if replay.get("result") != delegate.get("result"):
                    raise RuntimeError("worker delegate idempotency replay changed its receipt")
                workflow = delegate["result"]["workflow_id"]
                initial = request(stream, reader, 3, "worker_status", {
                    "workflow_id": workflow, "after_sequence": 0,
                })
                events = initial["result"]["events"]
                if [event["state"] for event in events] != ["waiting", "waiting"]:
                    raise RuntimeError(f"unexpected initial worker states: {events}")
                cancelled = request(stream, reader, 4, "worker_cancel", {
                    "workflow_id": workflow, "idempotency_key": "code-worker-cancel-e2e",
                    "reason": "E2E cancellation", "revoke_mutation_authority": True,
                })
                if not cancelled.get("ok"):
                    raise RuntimeError(cancelled)
                after_cancel = request(stream, reader, 5, "worker_status", {
                    "workflow_id": workflow, "after_sequence": initial["result"]["next_sequence"],
                })
                if {event["state"] for event in after_cancel["result"]["events"]} != {"cancelled"}:
                    raise RuntimeError("cancel did not revoke every pending task")
                blocked_delivery = request(stream, reader, 6, "worker_deliver", {
                    "workflow_id": workflow, "task_id": "review", "agent_id": "external-agent:review",
                    "attempt": 1, "fence": 2, "review_receipt_id": "review-e2e",
                    "idempotency_key": "delivery-e2e",
                })
                if blocked_delivery.get("ok") is not False:
                    raise RuntimeError("cancelled worker delivery was not rejected")
                reader.close()
                result.update({
                    "workflow_id": workflow,
                    "idempotent_delegate": True,
                    "initial_waiting_events": len(events),
                    "cancelled_tasks": len(after_cancel["result"]["events"]),
                    "delivery_blocked": True,
                    "hub_pid_before_restart": first.pid,
                })
            stop(first)
            first = None
            second = launch(loop_root, lock, endpoint)
            wait_for_socket(second, endpoint)
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as stream:
                stream.settimeout(5)
                stream.connect(str(endpoint))
                reader = stream.makefile("rb")
                persisted = request(stream, reader, 7, "worker_status", {
                    "workflow_id": workflow, "after_sequence": 2,
                })
                if not persisted.get("ok") or not persisted["result"]["events"]:
                    raise RuntimeError(f"worker state did not survive Hub restart: {persisted}")
                result.update({
                    "restart_persisted": True,
                    "hub_pid_after_restart": second.pid,
                })
                reader.close()
            stop(second)
            second = None
        result["exit_code"] = 0
    except Exception as exc:  # pragma: no cover - exercised by the command gate
        result.update({"exit_code": 1, "error": str(exc)})
        return_code = 1
    else:
        return_code = 0
    finally:
        if first is not None:
            stop(first)
        if second is not None:
            stop(second)
        result["duration_ms"] = round((time.perf_counter() - started_at) * 1000, 3)
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
