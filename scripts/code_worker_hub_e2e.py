#!/usr/bin/env python3
"""Real-process Code worker-protocol E2E against an external Loop Hub daemon.

This harness owns only external Hub process lifecycle. Worker requests are
issued by the Rust `SocketWorkerHubTransport` integration test, so the E2E
does not duplicate Code's wire client in Python. Remote delivery remains
fail-closed: a Hub-generated placeholder is not treated as a remote PR
confirmation.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import tempfile
import time
from typing import Any, Dict


def launch(loop_root: Path, lock: Path, endpoint: str, transport: str, shutdown: Path) -> subprocess.Popen[str]:
    env = os.environ.copy()
    prior_pythonpath = env.get("PYTHONPATH")
    env["PYTHONPATH"] = str(loop_root) + (os.pathsep + prior_pythonpath if prior_pythonpath else "")
    process_group = (
        {"creationflags": subprocess.CREATE_NEW_PROCESS_GROUP}
        if os.name == "nt"
        else {"start_new_session": True}
    )
    return subprocess.Popen(
        [sys.executable, "-c",
         "from simplicio_loop.hub_daemon import main; raise SystemExit(main())", "serve",
         "--lock", str(lock), "--endpoint", endpoint, "--transport", transport,
         "--shutdown-file", str(shutdown)],
        cwd=loop_root,
        env=env,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        **process_group,
    )

def wait_for_socket(process: subprocess.Popen[str], endpoint: str) -> None:
    deadline = time.monotonic() + 10
    address = endpoint.removeprefix("tcp://")
    host, port = address.rsplit(":", 1)
    while time.monotonic() < deadline:
        try:
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
                probe.settimeout(0.2)
                probe.connect((host, int(port)))
            return
        except OSError:
            pass
        if process.poll() is not None:
            stderr = process.stderr.read() if process.stderr else ""
            raise RuntimeError(f"Loop Hub exited before ready: {stderr[-2000:]}")
        time.sleep(0.02)
    raise TimeoutError("timed out waiting for external Loop Hub socket")


def stop(process: subprocess.Popen[str], shutdown: Path) -> None:
    if process.poll() is not None:
        return
    shutdown.write_text("shutdown\n", encoding="utf-8")
    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        process.kill()
        process.wait(timeout=5)


def run_adapter_test(
    code_root: Path,
    endpoint: str,
    phase: str,
    output: Path,
    workflow: str | None = None,
) -> Dict[str, Any]:
    """Run the real Rust Code adapter against the already-running Hub."""
    env = os.environ.copy()
    env.update(
        {
            "SIMPLICIO_WORKER_E2E_PHASE": phase,
            "SIMPLICIO_WORKER_E2E_ENDPOINT": endpoint,
            "SIMPLICIO_WORKER_E2E_OUTPUT": str(output),
        }
    )
    if workflow is not None:
        env["SIMPLICIO_WORKER_E2E_WORKFLOW"] = workflow
    command = [
        "cargo",
        "test",
        "--offline",
        "--locked",
        "-p",
        "simplicio-runtime-client",
        "--test",
        "external_worker_hub",
        "--",
        "--nocapture",
    ]
    completed = subprocess.run(
        command,
        cwd=code_root,
        env=env,
        capture_output=True,
        text=True,
        check=False,
    )
    if completed.returncode != 0:
        detail = (completed.stderr or completed.stdout)[-4000:]
        raise RuntimeError(f"Rust worker adapter E2E failed in {phase}: {detail}")
    receipt = json.loads(output.read_text(encoding="utf-8"))
    receipt["command"] = command
    receipt["stdout_tail"] = completed.stdout[-1000:]
    return receipt


def main(argv=None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--code-root", required=True)
    parser.add_argument("--loop-root", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args(argv)
    started_at = time.perf_counter()
    loop_root = Path(args.loop_root).resolve()
    code_root = Path(args.code_root).resolve()
    result: Dict[str, Any] = {
        "schema": "simplicio.code-worker-e2e/v1",
        "proof_kind": "rust_adapter_external_loop_hub_process",
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
            shutdown = root / "shutdown"
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as probe:
                probe.bind(("127.0.0.1", 0))
                endpoint = f"tcp://127.0.0.1:{probe.getsockname()[1]}"
            transport = "tcp"
            first = launch(loop_root, lock, endpoint, transport, shutdown)
            wait_for_socket(first, endpoint)
            initial_receipt = root / "initial.json"
            initial = run_adapter_test(code_root, endpoint, "initial", initial_receipt)
            workflow = initial["workflow_id"]
            result.update({
                "workflow_id": workflow,
                "adapter_initial": initial,
                "hub_pid_before_restart": first.pid,
            })
            stop(first, shutdown)
            first = None
            shutdown.unlink(missing_ok=True)
            second = launch(loop_root, lock, endpoint, transport, shutdown)
            wait_for_socket(second, endpoint)
            restart_receipt = root / "restart.json"
            restart = run_adapter_test(
                code_root, endpoint, "restart", restart_receipt, workflow=workflow
            )
            result.update({
                "adapter_restart": restart,
                "restart_persisted": restart["restart_persisted"],
                "hub_pid_after_restart": second.pid,
            })
            stop(second, shutdown)
            second = None
        result["exit_code"] = 0
    except Exception as exc:  # pragma: no cover - exercised by the command gate
        result.update({"exit_code": 1, "error": str(exc)})
        return_code = 1
    else:
        return_code = 0
    finally:
        if first is not None:
            stop(first, shutdown)
        if second is not None:
            stop(second, shutdown)
        result["duration_ms"] = round((time.perf_counter() - started_at) * 1000, 3)
        output = Path(args.output)
        output.parent.mkdir(parents=True, exist_ok=True)
        output.write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")
    print(json.dumps(result, sort_keys=True))
    return return_code


if __name__ == "__main__":
    raise SystemExit(main())
