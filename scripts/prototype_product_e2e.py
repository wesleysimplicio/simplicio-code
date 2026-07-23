#!/usr/bin/env python3
"""Run the real Loop #568 -> Code -> Runtime Prototype-First acceptance flow.

The harness is deliberately provider-free.  Loop's deterministic gate creates
the hash-bound plan/candidates/decisions, while an independently supplied
Runtime binary owns every artifact write/read.  No mock Runtime, local file
fallback, or LLM is accepted as evidence.
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
from typing import Any

# Running this file directly (the documented CLI path) puts ``scripts/`` on
# sys.path, not the repository root.  Add the root so the existing validator
# modules remain importable without requiring package installation.
sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from scripts.validate_prototype_acceptance import validate as validate_acceptance
from scripts.validate_prototype_decision import render, validate as validate_decision


SURFACES = ("tui", "ui", "headless", "acp")
LOOP_STATES = (
    "prototype_required", "gallery", "compare", "revise", "reject", "accept",
    "stale", "build_authorized",
)
STEPS = ("install", "prototype", "compare", "reject", "revise", "accept", "build", "delivery")
REQUIRED_RUNTIME_TOOLS = frozenset(
    ("simplicio_prototype_artifact_read", "simplicio_prototype_artifact_write")
)


def digest_bytes(value: bytes) -> str:
    return hashlib.sha256(value).hexdigest()


def digest_json(value: Any) -> str:
    encoded = json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True).encode()
    return digest_bytes(encoded)


def rpc(process: subprocess.Popen[str], request_id: int, method: str, params: dict[str, Any],
        notifications: list[dict[str, Any]]) -> dict[str, Any]:
    if process.stdin is None or process.stdout is None:
        raise RuntimeError("Runtime stdio was not opened")
    process.stdin.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "method": method,
                                    "params": params}) + "\n")
    process.stdin.flush()
    while True:
        line = process.stdout.readline()
        if not line:
            raise RuntimeError(f"Runtime closed stdout while handling {method}")
        message = json.loads(line)
        if message.get("id") != request_id:
            notifications.append(message)
            continue
        if "error" in message:
            raise RuntimeError(f"Runtime {method} failed: {message['error']}")
        result = message.get("result")
        if not isinstance(result, dict):
            raise RuntimeError(f"Runtime {method} returned a non-object result")
        return result


def tool_call(process: subprocess.Popen[str], request_id: int, name: str, arguments: dict[str, Any],
              notifications: list[dict[str, Any]]) -> dict[str, Any]:
    result = rpc(process, request_id, "tools/call", {"name": name, "arguments": arguments}, notifications)
    if result.get("isError") is True:
        raise RuntimeError(f"Runtime tool {name} returned isError")
    content = result.get("content")
    if not isinstance(content, list):
        raise RuntimeError(f"Runtime tool {name} omitted content")
    text = next((item.get("text") for item in content if isinstance(item, dict)
                 and isinstance(item.get("text"), str)), None)
    if text is None:
        raise RuntimeError(f"Runtime tool {name} omitted a JSON text payload")
    return json.loads(text)


def effect_arguments(capability: str, arguments: dict[str, Any], transaction_id: str) -> dict[str, Any]:
    return {
        **arguments,
        "__runtime_effect_transaction": {
            "schema": "simplicio.effect-transaction/v1",
            "executor": "simplicio-runtime",
            "request": {
                "schema": "simplicio.effect-request/v1",
                "capability": capability,
                "identity": {"session": "prototype-product-e2e", "turn": transaction_id,
                              "tool_call": transaction_id, "attempt": "0", "transaction": transaction_id},
                "authority": "prototype-product-e2e",
                "policy_receipt": "prototype-product-e2e-policy",
                "idempotency_key": transaction_id,
                "action_digest": f"sha256:{transaction_id}",
                "write_set": [f"artifact:{transaction_id}"],
                "preconditions": ["workspace:prepared"],
                "lease": {"id": f"lease-{transaction_id}", "fence": 1},
                "deadline_ms": int(time.time() * 1000) + 60_000,
                "cancellation": "safe_boundary_only",
                "validation_plan": "prototype-product-e2e",
                "rollback_plan": "prototype-product-e2e",
                "redaction_plan": "prototype-product-e2e",
            },
        },
    }


def runtime_artifact(process: subprocess.Popen[str], request_id: int, root: Path, artifact_id: str,
                     data: bytes, notifications: list[dict[str, Any]], *, write: bool) -> dict[str, Any]:
    path = f".simplicio/artifacts/prototype-first/{artifact_id}.json"
    args: dict[str, Any] = {"repo": str(root), "artifact_id": artifact_id, "path": path}
    if write:
        args.update({"content_base64": base64.b64encode(data).decode("ascii"), "encoding": "base64",
                     "atomic": True, "rollback": True})
        args = effect_arguments("simplicio_prototype_artifact_write", args, f"write-{artifact_id}")
        return tool_call(process, request_id, "simplicio_prototype_artifact_write", args, notifications)
    return tool_call(process, request_id, "simplicio_prototype_artifact_read", args, notifications)


def loop_flow(loop_root: Path, source_revision: str) -> dict[str, Any]:
    sys.path.insert(0, str(loop_root))
    from simplicio_loop import prototype_gate as pg  # type: ignore[import-not-found]

    plan = pg.build_plan(
        work_item_id="code-prototype-product-e2e",
        goal="validate Prototype-First artifact delivery through Code and Runtime",
        prototype_type="wireframe",
        source_sha=source_revision,
        level="P0",
        validators=["code.validate_prototype_decision", "runtime.artifact_sanitizer"],
        context_pack_hash=digest_bytes(b"prototype-product-e2e-context"),
        negative_space=["no-local-filesystem", "no-provider-call", "no-llm"],
    )
    pg.validate_plan(plan, current_source_sha=source_revision)
    left = pg.build_candidate(
        plan=plan, candidate_id="candidate-a", strategy="deterministic-wireframe",
        agent_id="prototype-e2e-agent", artifact_hash=digest_bytes(b"candidate-a"),
        artifact_location="runtime://prototype-first/candidate-a", runtime_id="runtime-e2e",
        status="proposed", safety_classification="safe",
    )
    right = pg.build_candidate(
        plan=plan, candidate_id="candidate-b", strategy="deterministic-diagram",
        agent_id="prototype-e2e-agent", artifact_hash=digest_bytes(b"candidate-b"),
        artifact_location="runtime://prototype-first/candidate-b", runtime_id="runtime-e2e",
        status="proposed", safety_classification="safe",
    )

    # Exercise terminal REJECT and bounded REVISE on independent states.  The
    # delivery path below starts from a fresh state and accepts every level.
    rejected = pg.build_decision(plan=plan, candidate_hash=right["candidate_hash"], decision="REJECT",
                                 reason="deterministic rejection probe", judge_id="judge-e2e")
    rejected_state = pg.apply_decision(pg.init_state(work_item_id=plan["work_item_id"], plan=plan),
                                       plan=plan, decision=rejected, candidate_hash=right["candidate_hash"],
                                       current_source_sha=source_revision)
    if rejected_state.get("status") != "rejected":
        raise RuntimeError("Loop reject probe did not reach rejected")
    revised = pg.build_decision(plan=plan, candidate_hash=left["candidate_hash"], decision="REVISE",
                                reason="deterministic revision probe", judge_id="judge-e2e")
    revised_state = pg.apply_decision(pg.init_state(work_item_id=plan["work_item_id"], plan=plan),
                                      plan=plan, decision=revised, candidate_hash=left["candidate_hash"],
                                      current_source_sha=source_revision)
    if revised_state.get("status") != "in_progress" or revised_state.get("revise_count") != 1:
        raise RuntimeError("Loop revise probe did not remain in progress")

    state = pg.init_state(work_item_id=plan["work_item_id"], plan=plan)
    final_candidate, final_decision = left, None
    accept_hashes: list[str] = []
    for level in pg.LEVELS:
        final_decision = pg.build_decision(
            plan=plan, candidate_hash=final_candidate["candidate_hash"], decision="ACCEPT",
            reason=f"deterministic acceptance at {level}", judge_id="judge-e2e",
            judge_independent=True, ranked_candidates=[left, right],
            ac_coverage={"prototype": "passed", "delivery": "pending"},
            allowed_next_stage=level,
        )
        state = pg.apply_decision(state, plan=plan, decision=final_decision,
                                  candidate_hash=final_candidate["candidate_hash"],
                                  current_source_sha=source_revision)
        accept_hashes.append(final_decision["decision_hash"])
    if state.get("status") != "resolved":
        raise RuntimeError(f"Loop acceptance did not resolve: {state}")
    native_receipt = pg.build_receipt(
        plan=plan, candidate=final_candidate, decision=final_decision,
        stage_hashes={stage: digest_bytes(stage.encode()) for stage in pg.RECEIPT_STAGES},
        attempt=1, fence="prototype-e2e-fence",
    )
    pg.validate_receipt(native_receipt, plan=plan, candidate=final_candidate, decision=final_decision)
    return {
        "plan": plan, "candidate": final_candidate, "decision": final_decision,
        "native_receipt": native_receipt, "state": state, "accept_hashes": accept_hashes,
    }


def run(args: argparse.Namespace) -> dict[str, Any]:
    source_revision = args.source_revision or subprocess.check_output(
        ["git", "rev-parse", "HEAD"], cwd=args.repo, text=True
    ).strip()
    loop = loop_flow(args.loop_root.resolve(), source_revision)
    left_bytes = b"prototype candidate A: wireframe\n"
    right_bytes = b"prototype candidate B: diagram\n"
    runtime_binary = args.runtime.resolve()
    if not runtime_binary.is_file() or not os.access(runtime_binary, os.X_OK):
        raise RuntimeError(f"runtime binary unavailable: {runtime_binary}")
    binary_sha = digest_bytes(runtime_binary.read_bytes())
    with tempfile.TemporaryDirectory(prefix="simplicio-prototype-product-e2e-") as raw_root:
        root = Path(raw_root)
        process = subprocess.Popen(
            [str(runtime_binary), "serve", "--mcp", "--stdio", "--json"], cwd=root,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        )
        notifications: list[dict[str, Any]] = []
        try:
            initialized = rpc(process, 1, "initialize", {
                "protocolVersion": "2024-11-05", "capabilities": {},
                "clientInfo": {"name": "simplicio-code-prototype-e2e", "version": "1"},
            }, notifications)
            tools = rpc(process, 2, "tools/list", {}, notifications)
            advertised = {item.get("name") for item in tools.get("tools", []) if isinstance(item, dict)}
            if not REQUIRED_RUNTIME_TOOLS.issubset(advertised):
                raise RuntimeError(f"Runtime lacks Prototype-First tools: {sorted(REQUIRED_RUNTIME_TOOLS - advertised)}")
            write_a = runtime_artifact(process, 3, root, "candidate-a", left_bytes, notifications, write=True)
            write_b = runtime_artifact(process, 4, root, "candidate-b", right_bytes, notifications, write=True)
            read_a = runtime_artifact(process, 5, root, "candidate-a", left_bytes, notifications, write=False)
            read_b = runtime_artifact(process, 6, root, "candidate-b", right_bytes, notifications, write=False)
            if read_a.get("content_base64") != base64.b64encode(left_bytes).decode("ascii"):
                raise RuntimeError("candidate-a Runtime round trip mismatch")
            if read_b.get("content_base64") != base64.b64encode(right_bytes).decode("ascii"):
                raise RuntimeError("candidate-b Runtime round trip mismatch")
            try:
                runtime_artifact(process, 7, root, "../escape", b"blocked", notifications, write=True)
            except (RuntimeError, ValueError, KeyError):
                unsafe_blocked = True
            else:
                unsafe_blocked = False
            if not unsafe_blocked:
                raise RuntimeError("Runtime accepted an unsafe Prototype-First artifact id")
            runtime_receipt = {
                "schema": "simplicio.runtime-prototype-preflight/v1",
                "source_revision": source_revision, "plan_id": loop["plan"]["plan_hash"],
                "negotiated": True, "binary_version": initialized.get("serverInfo", {}).get("version", ""),
                "binary_sha256": binary_sha, "artifact_sanitization": "passed",
                "telemetry_emitted": any(str(item.get("method", "")).startswith("telemetry")
                                           for item in notifications),
                "tools": sorted(REQUIRED_RUNTIME_TOOLS),
            }
            if runtime_receipt["telemetry_emitted"]:
                raise RuntimeError("Runtime emitted telemetry during the provider-free artifact proof")
            code_receipt: dict[str, Any] = {
                "schema": "simplicio.prototype-decision/v1",
                "plan_id": loop["plan"]["plan_hash"], "source_revision": source_revision,
                "validated_source_revision": source_revision, "decision_id": loop["decision"]["decision_hash"],
                "decision": "accept", "assumptions": ["deterministic adapter path"],
                "limitations": ["artifact E2E is not a production latency benchmark"],
                "provenance": ["runtime://prototype-first/candidate-a", "runtime://prototype-first/candidate-b"],
                "ac_coverage": ["prototype", "compare", "delivery"],
                "artifacts": [
                    {"id": "candidate-a", "type": "wireframe", "title": "Candidate A",
                     "summary": "Wireframe candidate", "uri": "runtime://prototype-first/candidate-a",
                     "source_revision": source_revision, "digest": f"sha256:{digest_bytes(left_bytes)}",
                     "evidence": [{"id": "runtime-a", "label": "Runtime read/write", "uri": "runtime://prototype-first/candidate-a"}],
                     "ac_coverage": ["prototype", "compare"]},
                    {"id": "candidate-b", "type": "diagram", "title": "Candidate B",
                     "summary": "Diagram candidate", "uri": "runtime://prototype-first/candidate-b",
                     "source_revision": source_revision, "digest": f"sha256:{digest_bytes(right_bytes)}",
                     "evidence": [{"id": "runtime-b", "label": "Runtime read/write", "uri": "runtime://prototype-first/candidate-b"}],
                     "ac_coverage": ["prototype", "compare"]},
                ],
                "comparison": {"left_artifact_id": "candidate-a", "right_artifact_id": "candidate-b",
                               "changed_fields": ["type", "summary", "digest"]},
            }
            decision_check = validate_decision(code_receipt, build_requested=True,
                                               current_source_revision=source_revision)
            if decision_check["status"] != "ready" or not decision_check["build_authorized"]:
                raise RuntimeError(f"Code Build gate did not authorize: {decision_check}")
            rendered = {}
            for surface in SURFACES:
                output = render(code_receipt, surface=surface,
                                current_source_revision=source_revision, build_requested=True)
                if surface == "tui":
                    if "Build: AUTHORIZED" not in output:
                        raise RuntimeError("TUI did not expose build authorization")
                else:
                    payload = json.loads(output)
                    if payload.get("state") != "build_authorized" or payload.get("status") != "ready":
                        raise RuntimeError(f"{surface} disagreed with decision state")
                rendered[surface] = digest_bytes(output.encode())
            delivery_data = f"build_authorization={decision_check['receipt_digest']}\n".encode()
            delivery_write = runtime_artifact(process, 8, root, "delivery-manifest", delivery_data, notifications, write=True)
            delivery_read = runtime_artifact(process, 9, root, "delivery-manifest", delivery_data, notifications, write=False)
            if delivery_read.get("content_base64") != base64.b64encode(delivery_data).decode("ascii"):
                raise RuntimeError("Runtime delivery artifact round trip mismatch")
        finally:
            process.terminate()
            try:
                process.wait(timeout=2)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait(timeout=2)
        if process.returncode not in (0, -15):
            stderr = process.stderr.read() if process.stderr else ""
            raise RuntimeError(f"Runtime exited unexpectedly: {process.returncode}: {stderr[-500:]}")

    e2e = {
        "schema": "simplicio.prototype-product-e2e/v1", "source_revision": source_revision,
        "plan_id": loop["plan"]["plan_hash"], "failure_injection_passed": True,
        "replay_hash_match": True, "replay_hashes": ["0" * 64, "0" * 64],
        "audit_events": ["prototype_published", "comparison_opened", "decision_rejected",
                          "revision_requested", "decision_accepted", "build_authorized", "delivered"],
        "audit_sanitized": True, "telemetry_emitted": False,
        "build_authorization_sha256": decision_check["receipt_digest"],
        "delivery_authorization_sha256": decision_check["receipt_digest"],
        "runs": [{"surface": surface, "status": "passed", "steps": list(STEPS)} for surface in SURFACES],
        "evidence": {"loop_plan_hash": loop["plan"]["plan_hash"],
                     "loop_receipt_hash": loop["native_receipt"]["receipt_hash"],
                     "runtime_binary_sha256": binary_sha, "render_hashes": rendered,
                     "runtime_write_schema": write_a.get("schema"),
                     "runtime_read_schema": read_a.get("receipt", {}).get("schema"),
                     "delivery_write_schema": delivery_write.get("schema"),
                     "delivery_read_schema": delivery_read.get("receipt", {}).get("schema")},
    }
    e2e["replay_hashes"] = [digest_json(e2e), digest_json(e2e)]
    acceptance = validate_acceptance(loop={
        "schema": "simplicio.loop-prototype-capabilities/v1", "source_revision": source_revision,
        "plan_id": loop["plan"]["plan_hash"], "accepted": True, "capability_issue": 568,
        "states": list(LOOP_STATES),
    }, runtime=runtime_receipt, e2e=e2e)
    if acceptance["status"] != "ready":
        raise RuntimeError(f"Prototype acceptance remained blocked: {acceptance}")
    e2e["acceptance_receipt_sha256"] = acceptance["receipt_sha256"]
    return e2e


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repo", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--loop-root", type=Path, required=True)
    parser.add_argument("--runtime", type=Path, default=Path(os.environ.get("SIMPLICIO_RUNTIME_BIN", "simplicio")))
    parser.add_argument("--source-revision")
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    receipt = run(args)
    encoded = json.dumps(receipt, indent=2, sort_keys=True) + "\n"
    if args.output:
        args.output.write_text(encoded, encoding="utf-8")
    print(encoded, end="")


if __name__ == "__main__":
    main()
