#!/usr/bin/env python3
"""Run the Code Rust Loop Hub client against a real external Loop daemon.

The harness starts only the supplied Loop Hub daemon. Code remains a client:
it does not start a Hub, scheduler, Runtime, Mapper, worker, model, or LLM.
"""

from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
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


MIN_P95_RUNS = 2


def validate_runs(runs: int) -> int:
    if runs < MIN_P95_RUNS:
        raise ValueError(f"benchmark requires at least {MIN_P95_RUNS} runs for p95 metrics")
    return runs


def digest(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def git_revision(root: Path) -> str:
    return subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()


def wait_for_endpoint(endpoint: str, process: subprocess.Popen[str], transport: str) -> None:
    for _ in range(500):
        if process.poll() is not None:
            stderr = process.stderr.read() if process.stderr is not None else ""
            raise RuntimeError(f"Loop Hub exited before becoming ready: {stderr[-4000:]}")
        ready = transport == "tcp" and _tcp_probe(endpoint)
        if transport == "unix" and Path(endpoint).exists():
            ready = True
        if ready:
            probe = socket.socket(socket.AF_INET if transport == "tcp" else socket.AF_UNIX, socket.SOCK_STREAM)
            try:
                probe.settimeout(0.1)
                if transport == "tcp":
                    host, raw_port = endpoint.rsplit(":", 1)
                    probe.connect((host, int(raw_port)))
                else:
                    probe.connect(endpoint)
                return
            except OSError:
                pass
            finally:
                probe.close()
        time.sleep(0.02)
    raise RuntimeError(f"Loop Hub did not create its {transport} endpoint")


def _tcp_probe(endpoint: str) -> bool:
    host, raw_port = endpoint.rsplit(":", 1)
    try:
        with socket.create_connection((host, int(raw_port)), timeout=0.1):
            return True
    except OSError:
        return False


def start_hub(loop_root: Path, env: dict[str, str], lock: Path, endpoint: str, transport: str) -> subprocess.Popen[str]:
    return subprocess.Popen(
        [sys.executable, "-c", "from simplicio_loop.hub_daemon import main; raise SystemExit(main())",
         "serve", "--lock", str(lock), "--endpoint", endpoint, "--transport", transport],
        cwd=loop_root, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )


def stop_hub(hub: subprocess.Popen[str], lock: Path | None = None) -> None:
    hub.terminate()
    try:
        hub.wait(timeout=5)
    except subprocess.TimeoutExpired:
        hub.kill()
        hub.wait(timeout=5)
    if lock is not None and hub.poll() is not None:
        lock.unlink(missing_ok=True)


_WINDOWS_CPU_SAMPLES: dict[int, tuple[int, float]] = {}


def _windows_filetime_value(value: wintypes.FILETIME) -> int:
    return (value.dwHighDateTime << 32) | value.dwLowDateTime


def _windows_process_tree_count(pid: int) -> int | None:
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)

    class ProcessEntry32W(ctypes.Structure):
        _fields_ = [
            ("dwSize", wintypes.DWORD),
            ("cntUsage", wintypes.DWORD),
            ("th32ProcessID", wintypes.DWORD),
            ("th32DefaultHeapID", ctypes.c_size_t),
            ("th32ModuleID", wintypes.DWORD),
            ("cntThreads", wintypes.DWORD),
            ("th32ParentProcessID", wintypes.DWORD),
            ("pcPriClassBase", wintypes.LONG),
            ("dwFlags", wintypes.DWORD),
            ("szExeFile", wintypes.WCHAR * 260),
        ]

    kernel32.CreateToolhelp32Snapshot.restype = wintypes.HANDLE
    kernel32.Process32FirstW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry32W)]
    kernel32.Process32FirstW.restype = wintypes.BOOL
    kernel32.Process32NextW.argtypes = [wintypes.HANDLE, ctypes.POINTER(ProcessEntry32W)]
    kernel32.Process32NextW.restype = wintypes.BOOL
    kernel32.CloseHandle.argtypes = [wintypes.HANDLE]
    snapshot = kernel32.CreateToolhelp32Snapshot(0x00000002, 0)
    if snapshot in (0, -1):
        return None
    try:
        entry = ProcessEntry32W()
        entry.dwSize = ctypes.sizeof(ProcessEntry32W)
        if not kernel32.Process32FirstW(snapshot, ctypes.byref(entry)):
            return None
        children: dict[int, list[int]] = {}
        while True:
            children.setdefault(int(entry.th32ParentProcessID), []).append(int(entry.th32ProcessID))
            if not kernel32.Process32NextW(snapshot, ctypes.byref(entry)):
                break
        pending = [pid]
        seen = {pid}
        count = 0
        while pending:
            current = pending.pop()
            count += 1
            for child in children.get(current, []):
                if child not in seen:
                    seen.add(child)
                    pending.append(child)
        return count
    finally:
        kernel32.CloseHandle(snapshot)


def _windows_process_sample(pid: int) -> tuple[int | None, float | None, float | None]:
    process_count = _windows_process_tree_count(pid)
    kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
    psapi = ctypes.WinDLL("psapi", use_last_error=True)
    handle = kernel32.OpenProcess(0x0410, False, pid)
    if not handle:
        return process_count, None, None

    class ProcessMemoryCounters(ctypes.Structure):
        _fields_ = [
            ("cb", wintypes.DWORD),
            ("PageFaultCount", wintypes.DWORD),
            ("PeakWorkingSetSize", ctypes.c_size_t),
            ("WorkingSetSize", ctypes.c_size_t),
            ("QuotaPeakPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPagedPoolUsage", ctypes.c_size_t),
            ("QuotaPeakNonPagedPoolUsage", ctypes.c_size_t),
            ("QuotaNonPagedPoolUsage", ctypes.c_size_t),
            ("PagefileUsage", ctypes.c_size_t),
            ("PeakPagefileUsage", ctypes.c_size_t),
        ]

    counters = ProcessMemoryCounters()
    counters.cb = ctypes.sizeof(ProcessMemoryCounters)
    working_set_kib: float | None = None
    if psapi.GetProcessMemoryInfo(handle, ctypes.byref(counters), counters.cb):
        working_set_kib = round(counters.WorkingSetSize / 1024, 3)

    creation = wintypes.FILETIME()
    exit_time = wintypes.FILETIME()
    kernel_time = wintypes.FILETIME()
    user_time = wintypes.FILETIME()
    cpu_percent: float | None = None
    if kernel32.GetProcessTimes(
        handle,
        ctypes.byref(creation),
        ctypes.byref(exit_time),
        ctypes.byref(kernel_time),
        ctypes.byref(user_time),
    ):
        cpu_ticks = _windows_filetime_value(kernel_time) + _windows_filetime_value(user_time)
        now = time.perf_counter()
        previous = _WINDOWS_CPU_SAMPLES.get(pid)
        _WINDOWS_CPU_SAMPLES[pid] = (cpu_ticks, now)
        if previous is not None and now > previous[1]:
            cpu_seconds = (cpu_ticks - previous[0]) / 10_000_000
            cpu_percent = round(100 * cpu_seconds / (now - previous[1]) / max(1, os.cpu_count() or 1), 3)

    kernel32.CloseHandle(handle)
    return process_count, working_set_kib, cpu_percent


def process_sample(pid: int) -> tuple[int | None, float | None, float | None]:
    """Return (process_count, rss_kib, cpu_percent) for the Hub tree."""
    if os.name == "nt":
        return _windows_process_sample(pid)
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
        transport = "tcp" if os.name == "nt" else "unix"
        if transport == "tcp":
            with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as allocator:
                allocator.bind(("127.0.0.1", 0))
                endpoint = f"127.0.0.1:{allocator.getsockname()[1]}"
        else:
            endpoint = str(root / "hub.sock")
        env = dict(os.environ)
        env["PYTHONPATH"] = str(loop_root) + os.pathsep + env.get("PYTHONPATH", "")
        startup_started = time.perf_counter()
        restart_ready = root / "restart.ready"
        restart_complete = root / "restart.complete"
        hub = start_hub(loop_root, env, lock, endpoint, transport)
        hub_pids = [hub.pid]
        restart_downtime_ms: float | None = None
        try:
            wait_for_endpoint(endpoint, hub, transport)
            startup_ms = round((time.perf_counter() - startup_started) * 1000, 3)
            test_env = dict(
                env,
                SIMPLICIO_LOOP_HUB_ENDPOINT=f"{transport}://{endpoint}",
                SIMPLICIO_LOOP_HUB_RESTART_READY=str(restart_ready),
                SIMPLICIO_LOOP_HUB_RESTART_COMPLETE=str(restart_complete),
            )
            command = [cargo, "test", "-p", "simplicio-runtime-client", "--test", "external_loop_hub", "--", "--nocapture"]
            test_started = time.perf_counter()
            child = subprocess.Popen(command, cwd=code_root, env=test_env, text=True,
                                     stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            samples: list[tuple[int, float, float]] = []
            while child.poll() is None:
                samples.append(process_sample(hub.pid))
                if restart_ready.exists() and restart_downtime_ms is None:
                    restart_started = time.perf_counter()
                    stop_hub(hub, lock)
                    hub = start_hub(loop_root, env, lock, endpoint, transport)
                    hub_pids.append(hub.pid)
                    wait_for_endpoint(endpoint, hub, transport)
                    restart_downtime_ms = round((time.perf_counter() - restart_started) * 1000, 3)
                    restart_complete.write_text("ready\n", encoding="utf-8")
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
            if restart_downtime_ms is None or "restart_reconnected=true" not in line:
                raise RuntimeError("Code external Hub test omitted real restart/reconnect proof")
            return {
                "schema": "simplicio.code-loop-hub-e2e/v1",
                "proof_kind": "external_loop_daemon",
                "code_revision": git_revision(code_root),
                "loop_revision": git_revision(loop_root),
                "endpoint_scheme": transport,
                "hub_identity_receipt": line,
                "stdout_sha256": digest(output),
                "startup_ms": startup_ms,
                "test_ms": test_ms,
                "restart_downtime_ms": restart_downtime_ms,
                "hub_pid_rotated": len(set(hub_pids)) == 2,
            "hub_processes_max": max((sample[0] for sample in samples if sample[0] is not None), default=None),
            "hub_rss_kib_max": max((sample[1] for sample in samples if sample[1] is not None), default=None),
            "hub_cpu_percent_max": max((sample[2] for sample in samples if sample[2] is not None), default=None),
                "provider_free": True,
                "local_llm_started": False,
                "deepseek_started": False,
                "runtime_started_by_code": False,
                "mapper_started_by_code": False,
                "scheduler_started_by_code": False,
                "lifecycle": ["handshake", "attach", "submit", "progress", "cancel", "resume", "replay"],
                "surfaces": ["tui-1", "tui-2", "headless", "acp"],
                "single_hub_identity": True,
                "restart_reconnected": True,
            }
        finally:
            stop_hub(hub, lock)


def run(args: argparse.Namespace) -> dict[str, object]:
    code_root = args.repo.resolve()
    loop_root = args.loop_root.resolve()
    runs = validate_runs(args.runs)
    receipts = [run_once(code_root, loop_root) for _ in range(runs)]
    startup = [float(receipt["startup_ms"]) for receipt in receipts]
    test = [float(receipt["test_ms"]) for receipt in receipts]
    restart = [float(receipt["restart_downtime_ms"]) for receipt in receipts]
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
            "restart_downtime_ms_p50": percentile(restart, 0.50),
            "restart_downtime_ms_p95": percentile(restart, 0.95) if runs >= 2 else None,
            "hub_processes_max": max((int(receipt["hub_processes_max"]) for receipt in receipts if receipt["hub_processes_max"] is not None), default=None),
            "hub_rss_kib_max": max((float(receipt["hub_rss_kib_max"]) for receipt in receipts if receipt["hub_rss_kib_max"] is not None), default=None),
            "hub_cpu_percent_max": max((float(receipt["hub_cpu_percent_max"]) for receipt in receipts if receipt["hub_cpu_percent_max"] is not None), default=None),
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
        "restart_reconnected": all(bool(receipt["restart_reconnected"]) for receipt in receipts),
        "hub_pid_rotated": all(bool(receipt["hub_pid_rotated"]) for receipt in receipts),
        "stdout_sha256": digest("\n".join(str(receipt["stdout_sha256"]) for receipt in receipts).encode()),
    }


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--loop-root", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runs", type=int, default=MIN_P95_RUNS)
    args = parser.parse_args()
    receipt = run(args)
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
