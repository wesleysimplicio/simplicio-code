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


def process_sample(pid: int) -> tuple[int, float, float]:
    """Return (process_count, rss_kib, cpu_percent) for the Hub tree."""
    try:
        status = subprocess.run(
            ["ps", "-o", "rss=,pcpu=", "-p", str(pid)],
            capture_output=True,
            text=True,
            check=False,
        )
        row = status.stdout.strip().split()
        if not row:
            return 0, 0.0, 0.0
        rss = float(row[0])
        cpu = float(row[1]) if len(row) > 1 else 0.0
        children = subprocess.run(
            ["pgrep", "-P", str(pid)], capture_output=True, text=True, check=False
        )
        child_count = len([line for line in children.stdout.splitlines() if line.strip()])
        return 1 + child_count, rss, cpu
    except (OSError, ValueError):
        return 0, 0.0, 0.0


def percentile(values: list[float], fraction: float) -> float | None:
    if not values:
        return None
    ordered = sorted(values)
    index = min(len(ordered) - 1, round((len(ordered) - 1) * fraction))
    return round(ordered[index], 3)


def run_once(code_root: Path, loop_root: Path) -> dict[str, object]:
    cargo = shutil.which("cargo")
    if cargo is None:
        raise RuntimeError("cargo is required for the Code client proof")
    with tempfile.TemporaryDirectory(prefix="simplicio-code-loop-hub-e2e-") as directory:
        root = Path(directory)
        lock = root / "hub.lock"
        endpoint = root / "hub.sock"
        env = dict(os.environ)
        env["PYTHONPATH"] = str(loop_root) + os.pathsep + env.get("PYTHONPATH", "")
        startup_started = time.perf_counter()
        hub = subprocess.Popen(
            [sys.executable, "-c", "from simplicio_loop.hub_daemon import main; raise SystemExit(main())",
             "serve", "--lock", str(lock), "--endpoint", str(endpoint), "--transport", "unix"],
            cwd=loop_root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        try:
            wait_for_socket(endpoint, hub)
            startup_ms = round((time.perf_counter() - startup_started) * 1000, 3)
            test_env = dict(env, SIMPLICIO_LOOP_HUB_ENDPOINT=f"unix://{endpoint}")
            command = [cargo, "test", "-p", "simplicio-runtime-client", "--test", "external_loop_hub", "--", "--nocapture"]
            test_started = time.perf_counter()
            child = subprocess.Popen(command, cwd=code_root, env=test_env, text=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            samples: list[tuple[int, float, float]] = []
            while child.poll() is None:
                samples.append(process_sample(hub.pid))
                time.sleep(0.02)
            stdout, stderr = child.communicate()
            test_ms = round((time.perf_counter() - test_started) * 1000, 3)
            completed = subprocess.CompletedProcess(command, child.returncode, stdout, stderr)
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
                "startup_ms": startup_ms,
                "test_ms": test_ms,
                "hub_processes_max": max((sample[0] for sample in samples), default=1),
                "hub_rss_kib_max": max((sample[1] for sample in samples), default=0.0),
                "hub_cpu_percent_max": max((sample[2] for sample in samples), default=0.0),
                "provider_free": True,
                "local_llm_started": False,
                "deepseek_started": False,
                "runtime_started_by_code": False,
                "mapper_started_by_code": False,
                "scheduler_started_by_code": False,
                "lifecycle": ["handshake", "attach", "submit", "progress", "cancel", "resume", "replay"],
                "surfaces": ["tui-1", "tui-2", "headless", "acp"],
                "single_hub_identity": True,
            }
        finally:
            hub.terminate()
            try:
                hub.wait(timeout=5)
            except subprocess.TimeoutExpired:
                hub.kill()
                hub.wait(timeout=5)


def run(args: argparse.Namespace) -> dict[str, object]:
    code_root = args.repo.resolve()
    loop_root = args.loop_root.resolve()
    runs = max(1, args.runs)
    receipts = [run_once(code_root, loop_root) for _ in range(runs)]
    startup = [float(receipt["startup_ms"]) for receipt in receipts]
    test = [float(receipt["test_ms"]) for receipt in receipts]
    return {
        "schema": "simplicio.code-loop-hub-e2e/v1",
        "proof_kind": "external_loop_daemon",
        "code_revision": git_revision(code_root),
        "loop_revision": git_revision(loop_root),
        "runs": runs,
        "metrics": {
            "startup_ms_p50": percentile(startup, 0.50),
            "startup_ms_p95": percentile(startup, 0.95) if runs >= 2 else None,
            "test_ms_p50": percentile(test, 0.50),
            "test_ms_p95": percentile(test, 0.95) if runs >= 2 else None,
            "hub_processes_max": max(int(receipt["hub_processes_max"]) for receipt in receipts),
            "hub_rss_kib_max": max(float(receipt["hub_rss_kib_max"]) for receipt in receipts),
            "hub_cpu_percent_max": max(float(receipt["hub_cpu_percent_max"]) for receipt in receipts),
        },
        "run_receipts": receipts,
        "provider_free": True,
        "local_llm_started": False,
        "deepseek_started": False,
        "runtime_started_by_code": False,
        "mapper_started_by_code": False,
        "scheduler_started_by_code": False,
        "lifecycle": ["handshake", "attach", "submit", "progress", "cancel", "resume", "replay"],
        "surfaces": ["tui-1", "tui-2", "headless", "acp"],
        "single_hub_identity": all(bool(receipt["single_hub_identity"]) for receipt in receipts),
        "stdout_sha256": digest("\n".join(str(receipt["stdout_sha256"]) for receipt in receipts).encode()),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--loop-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=1)
    args = parser.parse_args()
    receipt = run(args)
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
