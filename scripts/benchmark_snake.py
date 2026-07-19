#!/usr/bin/env python3
"""Reproducible Simplicio Code versus Hermes Snake benchmark."""
from __future__ import annotations
import argparse, json, os, platform, re, shlex, shutil, subprocess, sys, tempfile, time
from pathlib import Path
from typing import Any

PROMPT = """Create a complete Snake game in React 18 + TypeScript + Vite.
Requirements: arrow-key controls, collisions and game-over; score and
localStorage high score; top-10 scoreboard; restart; responsive UI; automated
tests for core rules; package scripts for test and build. Do not edit outside
the assigned workspace."""
ALIASES = {
    "prompt_tokens": ("prompt_tokens", "input_tokens"),
    "completion_tokens": ("completion_tokens", "output_tokens"),
    "total_tokens": ("total_tokens",),
}

def ms() -> int:
    return time.monotonic_ns() // 1_000_000

def write_json(path: Path, value: Any) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8")

def command(template: str, workspace: Path, model: str) -> list[str]:
    return [x.format(workspace=str(workspace), model=model, prompt=PROMPT)
            for x in shlex.split(template)]

def proc_stats(pid: int) -> tuple[int, int]:
    rss = ticks = 0
    try:
        for line in Path(f"/proc/{pid}/status").read_text().splitlines():
            if line.startswith("VmHWM:"):
                rss = int(line.split()[1]) * 1024
                break
        fields = Path(f"/proc/{pid}/stat").read_text().split()
        ticks = int(fields[13]) + int(fields[14])
    except (FileNotFoundError, PermissionError, ValueError, IndexError):
        pass
    return rss, ticks

def usage(text: str) -> dict[str, int] | None:
    found: dict[str, int] = {}
    for raw in re.findall(r"\{[^{}]{0,3000}\}", text, re.S):
        try:
            stack = [json.loads(raw)]
        except json.JSONDecodeError:
            continue
        while stack:
            item = stack.pop()
            if isinstance(item, dict):
                for target, keys in ALIASES.items():
                    if target not in found:
                        for key in keys:
                            if isinstance(item.get(key), (int, float)):
                                found[target] = int(item[key])
                                break
                stack.extend(item.values())
            elif isinstance(item, list):
                stack.extend(item)
    if not found:
        return None
    if "total_tokens" not in found and {"prompt_tokens", "completion_tokens"} <= found.keys():
        found["total_tokens"] = found["prompt_tokens"] + found["completion_tokens"]
    return found

def product_check(workspace: Path) -> dict[str, Any]:
    result: dict[str, Any] = {
        "package_json": False, "react_vite": False, "snake_sources": [],
        "tests": "NOT_RUN", "build": "NOT_RUN", "browser": "UNVERIFIED"
    }
    package = workspace / "package.json"
    if not package.exists():
        return result
    result["package_json"] = True
    try:
        data = json.loads(package.read_text())
    except (OSError, json.JSONDecodeError):
        return result
    deps = {**data.get("dependencies", {}), **data.get("devDependencies", {})}
    result["react_vite"] = "react" in deps and "vite" in deps
    for root in (workspace / "src", workspace / "app"):
        if root.exists():
            result["snake_sources"] += [
                str(p.relative_to(workspace)) for p in root.rglob("*")
                if p.is_file() and re.search(r"snake|game|score|board|collision", p.name, re.I)
            ]
    return result

def npm_check(workspace: Path, script: str, timeout: int) -> tuple[str, str]:
    try:
        p = subprocess.run(["npm", "run", script, "--if-present"], cwd=workspace,
                           text=True, capture_output=True, timeout=timeout, check=False)
    except FileNotFoundError:
        return "UNVERIFIED", "npm unavailable"
    except subprocess.TimeoutExpired:
        return "FAIL", "timeout"
    if p.returncode == 0:
        return "PASS", p.stdout[-1000:]
    if not p.stdout and not p.stderr:
        return "UNVERIFIED", f"npm script {script} is missing"
    return "FAIL", (p.stderr or p.stdout)[-1000:]

def run_agent(name: str, template: str, root: Path, model: str, timeout: int,
              events: list[dict[str, Any]], receipt: str | None) -> dict[str, Any]:
    workspace = root / name
    workspace.mkdir()
    argv = command(template, workspace, model)
    events.append({"event": "agent_started", "agent": name, "argv": argv, "ts_ms": ms()})
    started = ms()
    try:
        p = subprocess.Popen(argv, cwd=workspace, stdout=subprocess.PIPE,
                             stderr=subprocess.STDOUT, text=True,
                             env={**os.environ, "SIMPLICIO_BENCHMARK": "1"})
    except (OSError, ValueError) as exc:
        return {"agent": name, "status": "FAIL", "error": str(exc), "exit_code": None}
    peak_rss = peak_ticks = 0
    deadline = time.monotonic() + timeout
    timed_out = False
    while p.poll() is None and time.monotonic() < deadline:
        rss, ticks = proc_stats(p.pid)
        peak_rss, peak_ticks = max(peak_rss, rss), max(peak_ticks, ticks)
        time.sleep(0.05)
    if p.poll() is None:
        timed_out = True
        p.kill()
    output = p.communicate(timeout=5)[0]
    (workspace / "agent-output.log").write_text(output, encoding="utf-8")
    product = product_check(workspace)
    if product["package_json"]:
        product["tests"], product["tests_detail"] = npm_check(workspace, "test", timeout)
        product["build"], product["build_detail"] = npm_check(workspace, "build", timeout)
    runtime = {"status": "UNVERIFIED", "receipt": None}
    if name == "simplicio":
        path = Path(receipt) if receipt else workspace / "runtime-receipt.json"
        if path.exists():
            try:
                data = json.loads(path.read_text())
                pure = (data.get("server_name", "").lower() == "simplicio"
                        and data.get("fallback_used") is False
                        and bool(data.get("operations")))
                runtime = {"status": "PASS" if pure else "FAIL", "receipt": str(path)}
            except (OSError, json.JSONDecodeError):
                runtime = {"status": "FAIL", "receipt": str(path)}
    status = "FAIL" if timed_out or p.returncode != 0 else "PASS"
    if not product["package_json"] or not product["snake_sources"]:
        status = "FAIL"
    if name == "simplicio" and status == "PASS" and runtime["status"] != "PASS":
        status = "UNVERIFIED"
    result = {
        "agent": name, "status": status, "exit_code": p.returncode,
        "timed_out": timed_out, "wall_ms": ms() - started,
        "cpu_ticks_linux": peak_ticks or None, "rss_peak_bytes": peak_rss or None,
        "tokens": usage(output), "token_usage_status": "MEASURED" if usage(output) else "UNAVAILABLE",
        "runtime_gate": runtime, "quality": {"product": product},
        "output_log": str(workspace / "agent-output.log"), "command": argv,
    }
    events.append({"event": "agent_finished", "agent": name, "status": status,
                   "wall_ms": result["wall_ms"], "ts_ms": ms()})
    return result

def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("--simplicio-cmd", default="simplicio-code -p {prompt} --output-format json")
    ap.add_argument("--hermes-cmd", default="hermes --model {model} --accept-hooks --prompt {prompt}")
    ap.add_argument("--model", default=os.environ.get("MODEL", ""))
    ap.add_argument("--workspace", type=Path)
    ap.add_argument("--output", type=Path, default=Path(".simplicio/benchmarks/snake"))
    ap.add_argument("--timeout", type=int, default=900)
    ap.add_argument("--repetitions", type=int, default=1)
    ap.add_argument("--runtime-receipt")
    args = ap.parse_args()
    if not args.model:
        ap.error("--model or MODEL is required for a comparable run")
    if args.repetitions < 1:
        ap.error("--repetitions must be >= 1")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    events: list[dict[str, Any]] = []
    runs = []
    for repetition in range(args.repetitions):
        root = Path(tempfile.mkdtemp(prefix=f"snake-{repetition}-",
                                      dir=str(args.workspace.resolve()) if args.workspace else None))
        for name, template in (("simplicio", args.simplicio_cmd), ("hermes", args.hermes_cmd)):
            runs.append({"repetition": repetition, **run_agent(
                name, template, root, args.model, args.timeout, events, args.runtime_receipt)})
    status = "PASS" if all(r.get("status") == "PASS" for r in runs) else (
        "UNVERIFIED" if any(r.get("status") == "UNVERIFIED" for r in runs) else "FAIL")
    report = {
        "schema": "simplicio.snake-benchmark/v1", "status": status,
        "challenge": {"prompt": PROMPT, "model": args.model, "repetitions": args.repetitions},
        "environment": {"platform": platform.platform(), "python": sys.version.split()[0],
                        "node": shutil.which("node"), "npm": shutil.which("npm")},
        "results": runs,
        "limitations": [
            "Token usage is UNAVAILABLE unless the provider emits usage fields.",
            "Browser behavior is UNVERIFIED; this harness does not fake browser evidence.",
            "Pure Runtime is UNVERIFIED without a valid runtime-receipt.json.",
        ],
    }
    write_json(output / "benchmark-result.json", report)
    (output / "events.jsonl").write_text(
        "\n".join(json.dumps(x, sort_keys=True) for x in events) + "\n", encoding="utf-8")
    write_json(output / "cost-ledger.json", {
        "schema": "simplicio.cost-ledger/v1", "model": args.model,
        "rows": [{"agent": r["agent"], "tokens": r.get("tokens"),
                  "token_status": r.get("token_usage_status"),
                  "wall_ms": r.get("wall_ms"), "rss_peak_bytes": r.get("rss_peak_bytes")}
                 for r in runs],
        "paid_tokens_claimable": all(r.get("token_usage_status") == "MEASURED" for r in runs),
    })
    lines = ["# Snake benchmark", "", f"- status: {status}",
             f"- model: {args.model}", "", "| Agent | Status | Wall ms | RSS | Tokens | Runtime |",
             "|---|---|---:|---:|---:|---|"]
    for r in runs:
        lines.append("| {agent} | {status} | {wall_ms} | {rss} | {tokens} | {runtime} |".format(
            agent=r["agent"], status=r["status"], wall_ms=r.get("wall_ms"),
            rss=r.get("rss_peak_bytes") or "UNAVAILABLE",
            tokens=(r.get("tokens") or {}).get("total_tokens", "UNAVAILABLE"),
            runtime=r.get("runtime_gate", {}).get("status", "N/A")))
    (output / "benchmark-report.md").write_text("\n".join(lines) + "\n", encoding="utf-8")
    print(json.dumps({"status": status, "output": str(output)}, indent=2))
    return 0 if status == "PASS" else 1

if __name__ == "__main__":
    raise SystemExit(main())
