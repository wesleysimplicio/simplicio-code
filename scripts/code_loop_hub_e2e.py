#!/usr/bin/env python3
"""Run the Code Rust Loop Hub client against a real external Loop daemon.

The harness starts only the supplied Loop Hub daemon. Code remains a client:
it does not start a Hub, scheduler, Runtime, Mapper, worker, model, or LLM.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def git_revision(root: Path) -> str:
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()


def wait_for_socket(path: Path, process: subprocess.Popen[str]) -> None:
    for _ in range(500):
        if path.exists():
            return
        if process.poll() is not None:
            raise RuntimeError("Loop Hub exited before creating its socket")
        time.sleep(0.02)
    raise RuntimeError("Loop Hub did not create its socket")


def run(args: argparse.Namespace) -> dict[str, object]:
    code_root = args.repo.resolve()
    loop_root = args.loop_root.resolve()
    cargo = shutil.which("cargo")
    if cargo is None:
        raise RuntimeError("cargo is required for the Code client proof")
    with tempfile.TemporaryDirectory(prefix="simplicio-code-loop-hub-e2e-") as directory:
        root = Path(directory)
        lock = root / "hub.lock"
        endpoint = root / "hub.sock"
        env = dict(os.environ)
        env["PYTHONPATH"] = str(loop_root) + os.pathsep + env.get("PYTHONPATH", "")
        hub = subprocess.Popen(
            [sys.executable, "-c", "from simplicio_loop.hub_daemon import main; raise SystemExit(main())",
             "serve", "--lock", str(lock), "--endpoint", str(endpoint), "--transport", "unix"],
            cwd=loop_root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            wait_for_socket(endpoint, hub)
            test_env = dict(env, SIMPLICIO_LOOP_HUB_ENDPOINT=f"unix://{endpoint}")
            command = [cargo, "test", "-p", "simplicio-runtime-client", "--test", "external_loop_hub", "--", "--nocapture"]
            completed = subprocess.run(command, cwd=code_root, env=test_env, text=True,
                                       capture_output=True, check=False)
            output = (completed.stdout + "\n" + completed.stderr).encode()
            if completed.returncode != 0:
                raise RuntimeError(f"Code external Hub test failed ({completed.returncode}): {output.decode()[-4000:]}")
            line = next((line for line in output.decode().splitlines() if line.startswith("hub_id=")), "")
            if not line:
                raise RuntimeError("Code external Hub test omitted identity receipt")
            return {
                "schema": "simplicio.code-loop-hub-e2e/v1",
                "proof_kind": "external_loop_daemon",
                "code_revision": git_revision(code_root),
                "loop_revision": git_revision(loop_root),
                "endpoint_scheme": "unix",
                "hub_identity_receipt": line,
                "stdout_sha256": digest(output),
                "provider_free": True,
                "local_llm_started": False,
                "deepseek_started": False,
                "runtime_started_by_code": False,
                "mapper_started_by_code": False,
                "scheduler_started_by_code": False,
                "lifecycle": ["handshake", "attach", "submit", "progress", "cancel"],
            }
        finally:
            hub.terminate()
            try:
                hub.wait(timeout=5)
            except subprocess.TimeoutExpired:
                hub.kill()
                hub.wait(timeout=5)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--loop-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    receipt = run(args)
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
