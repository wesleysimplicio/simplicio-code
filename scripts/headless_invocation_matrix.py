#!/usr/bin/env python3
"""Plan/check the headless permission invocation matrix (#68).

Without a model/provider this script is an offline matrix generator, suitable
for CI and code review. With ``--binary`` it executes each combination with a
bounded timeout and records whether the process terminates; the caller chooses
the provider/model environment explicitly.
"""
from __future__ import annotations

import argparse
import itertools
import json
import os
import subprocess
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from dataclasses import asdict, dataclass
from urllib.parse import urlparse


# Windows fatal process codes are unsigned DWORDs when surfaced by Python.
# They must never be mistaken for an expected CLI error just because the
# process terminated.
FATAL_PROCESS_CODES = {0xC00000FD, 0xC0000005, 0xC0000409}
PERMISSION_MODES = ("default", "acceptEdits", "auto", "dontAsk", "bypassPermissions", "plan")


def _tail(value: str) -> str:
    return value[-2000:]


@dataclass(frozen=True)
class Case:
    name: str
    prompt_args: tuple[str, ...]
    approval_args: tuple[str, ...]
    tty: bool

    def to_dict(self) -> dict[str, object]:
        value = asdict(self)
        value["prompt_args"] = list(self.prompt_args)
        value["approval_args"] = list(self.approval_args)
        return value


def build_cases() -> list[Case]:
    cases = []
    for prompt, approval, tty in itertools.product(
        (("-p", "ping"), ("ping",)),
        (("--always-approve",), *[("--permission-mode", mode) for mode in PERMISSION_MODES]),
        (False, True),
    ):
        approval_name = approval[1] if approval[0] == "--permission-mode" else "always-approve"
        name = "-".join(["single" if prompt[0] == "-p" else "positional", approval_name, "tty" if tty else "no-tty"])
        cases.append(Case(name, prompt, approval, tty))
    return cases


class _MockProvider(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self) -> None:  # noqa: N802 - stdlib handler API
        path = urlparse(self.path).path
        responses = {
            "/v1/models": {"object": "list", "data": [{"id": "test-model"}]},
            "/v1/settings": {"allow_access": True},
            "/v1/user": {"subscriptionTier": "pro"},
        }
        self._json(200, responses[path]) if path in responses else self._json(404, {"error": "not found"})

    def do_POST(self) -> None:  # noqa: N802 - stdlib handler API
        path = urlparse(self.path).path
        if path not in {"/v1/chat/completions", "/v1/responses", "/v1/messages"}:
            self._json(404, {"error": "not found"})
            return
        if path == "/v1/chat/completions":
            events = [
                {"id": "chatcmpl-matrix", "object": "chat.completion.chunk", "created": 1, "model": "test-model", "choices": [{"index": 0, "delta": {"role": "assistant", "content": "ok"}, "finish_reason": "stop"}]},
                {"id": "chatcmpl-matrix", "object": "chat.completion.chunk", "created": 1, "model": "test-model", "choices": [], "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2}},
            ]
        elif path == "/v1/responses":
            events = [
                {"type": "response.created", "sequence_number": 0, "response": {"id": "resp-matrix", "object": "response", "created_at": 1, "model": "test-model", "status": "in_progress", "output": []}},
                {"type": "response.output_text.delta", "sequence_number": 1, "item_id": "item-matrix", "output_index": 0, "content_index": 0, "delta": "ok"},
                {"type": "response.completed", "sequence_number": 2, "response": {"id": "resp-matrix", "object": "response", "created_at": 1, "model": "test-model", "status": "completed", "output": [{"type": "message", "id": "msg-matrix", "role": "assistant", "status": "completed", "content": [{"type": "output_text", "text": "ok", "annotations": []}]}], "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2, "input_tokens_details": {"cached_tokens": 0}, "output_tokens_details": {"reasoning_tokens": 0}}}},
            ]
        else:
            events = [
                {"type": "message_start", "message": {"id": "msg-matrix", "type": "message", "role": "assistant", "content": [], "model": "test-model", "stop_reason": None, "usage": {"input_tokens": 1, "output_tokens": 0}},},
                {"type": "content_block_start", "index": 0, "content_block": {"type": "text", "text": ""}},
                {"type": "content_block_delta", "index": 0, "delta": {"type": "text_delta", "text": "ok"}},
                {"type": "content_block_stop", "index": 0},
                {"type": "message_delta", "delta": {"stop_reason": "end_turn"}, "usage": {"output_tokens": 1, "input_tokens": 1}},
                {"type": "message_stop"},
            ]
        payload = b"".join(f"data: {json.dumps(event)}\n\n".encode("utf-8") for event in events)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)

    def _json(self, status: int, value: dict[str, object]) -> None:
        payload = json.dumps(value).encode("utf-8")
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(payload)

    def log_message(self, _format: str, *_args: object) -> None:
        return


def _start_mock_provider() -> tuple[ThreadingHTTPServer, str]:
    server = ThreadingHTTPServer(("127.0.0.1", 0), _MockProvider)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    return server, f"http://127.0.0.1:{server.server_port}/v1"


def execute(binary: str, case: Case, timeout_seconds: float, env: dict[str, str] | None = None, cwd: str | None = None) -> dict[str, object]:
    command = [binary, *case.approval_args, "--output-format", "json", *case.prompt_args]
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout_seconds, check=False, env=env, cwd=cwd)
        code = completed.returncode & 0xFFFFFFFF
        crashed = code in FATAL_PROCESS_CODES
        return {
            "case": case.to_dict(), "command": command, "terminated": True,
            "returncode": completed.returncode, "outcome": "crash" if crashed else "completed",
            "reason_code": f"process_crash_{code:08x}" if crashed else None,
            "stdout_tail": _tail(completed.stdout), "stderr_tail": _tail(completed.stderr),
        }
    except subprocess.TimeoutExpired as exc:
        stdout = exc.stdout if isinstance(exc.stdout, str) else ""
        stderr = exc.stderr if isinstance(exc.stderr, str) else ""
        return {"case": case.to_dict(), "command": command, "terminated": False, "returncode": 124, "outcome": "timeout", "reason_code": "timeout", "stdout_tail": _tail(stdout or ""), "stderr_tail": _tail(stderr or "")}
    except OSError as exc:
        return {"case": case.to_dict(), "command": command, "terminated": True, "returncode": 127, "outcome": "not_executed", "reason_code": "binary_unavailable", "error": str(exc)}


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Headless permission invocation matrix")
    parser.add_argument("--binary", help="execute the matrix against this binary")
    parser.add_argument("--timeout-seconds", type=float, default=20.0)
    parser.add_argument("--mock-provider", action="store_true", help="run with a local provider and isolated test environment")
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    if args.timeout_seconds <= 0:
        parser.error("--timeout-seconds must be positive")
    cases = build_cases()
    if args.mock_provider and not args.binary:
        parser.error("--mock-provider requires --binary")
    provider = None
    home = None
    env = None
    try:
        if args.mock_provider:
            provider, provider_url = _start_mock_provider()
            home = tempfile.TemporaryDirectory(prefix="simplicio-headless-")
            env = os.environ.copy()
            env.update({
                "GROK_CLI_CHAT_PROXY_BASE_URL": provider_url,
                "GROK_XAI_API_BASE_URL": provider_url,
                "XAI_API_KEY": "test-key-for-ci",
                "GROK_HOME": os.path.join(home.name, ".grok"),
                "GROK_TELEMETRY_ENABLED": "false",
                "GROK_FEEDBACK_ENABLED": "false",
                "GROK_TRACE_UPLOAD": "false",
                "GROK_INSTRUMENTATION": "disabled",
                "GROK_DISABLE_AUTOUPDATER": "1",
                "GROK_MANAGED_MCPS_ENABLED": "false",
                "GROK_MANAGED_MCP_GATEWAY_TOOLS_ENABLED": "false",
                "GROK_MCP_AUTO_RESTART": "false",
                "GROK_MCP_STARTUP_TIMEOUT_SECS": "1",
                "GROK_MCP_LIVENESS_WATCHERS": "false",
                "GROK_MCP_PUSH_SERVER_STATUS": "false",
                "GROK_MCP_RECURSIVE_CONFIG_WATCH": "false",
            })
            for vendor in ("CURSOR", "CLAUDE"):
                for surface in ("SKILLS", "RULES", "AGENTS", "MCPS", "HOOKS", "SESSIONS"):
                    env[f"GROK_{vendor}_{surface}_ENABLED"] = "false"
        results = [execute(args.binary, case, args.timeout_seconds, env, home.name if home else None) for case in cases] if args.binary else [
            {"case": case.to_dict(), "planned": True} for case in cases
        ]
    finally:
        if provider:
            provider.shutdown()
            provider.server_close()
        if home:
            home.cleanup()
    report = {
        "schema": "simplicio.headless-invocation-matrix/v1",
        "offline": args.binary is None,
        "mock_provider": args.mock_provider,
        "case_count": len(results),
        "results": results,
        "all_terminated": all(result.get("terminated", True) for result in results),
        "all_healthy": all(result.get("outcome") == "completed" for result in results) if args.binary else None,
    }
    print(json.dumps(report, indent=2, sort_keys=True) if args.json else json.dumps(report, indent=2))
    return 0 if report["all_terminated"] and (report["all_healthy"] is not False) else 2


if __name__ == "__main__":
    raise SystemExit(main())
