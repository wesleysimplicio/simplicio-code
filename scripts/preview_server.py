#!/usr/bin/env python3
"""Bounded loopback preview for generated local reports.

The Browser pane owns browser automation.  This module only supplies the
Code-side workaround for ``file://`` reports: stage exactly one file in a
temporary directory and serve it from an ephemeral loopback port.
"""

from __future__ import annotations

import argparse
from contextlib import contextmanager
from dataclasses import dataclass
import json
from pathlib import Path
import shutil
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from functools import partial
from typing import Iterator
from urllib.parse import unquote, urlparse


SCHEMA = "simplicio.code-preview/v1"


@dataclass(frozen=True)
class PreviewTarget:
    """The only URL and lifecycle data exposed to an external browser."""

    source: Path
    staged: Path
    url: str


def _file_uri_path(value: str) -> Path:
    parsed = urlparse(value)
    if parsed.scheme.lower() != "file":
        raise ValueError("preview target must use a file:// URI")
    raw = unquote(parsed.path)
    if parsed.netloc:
        raw = f"//{parsed.netloc}{raw}"
    elif len(raw) >= 3 and raw[0] == "/" and raw[2] == ":":
        raw = raw[1:]
    return Path(raw)


def _quiet_handler(*args: object, **kwargs: object) -> SimpleHTTPRequestHandler:
    class QuietHandler(SimpleHTTPRequestHandler):
        def log_message(self, format: str, *message: object) -> None:
            return

    return QuietHandler(*args, **kwargs)


@contextmanager
def serve_file(path: str | Path) -> Iterator[PreviewTarget]:
    """Serve one copied report on ``127.0.0.1`` and tear it down on exit."""

    source = Path(path).expanduser().resolve()
    if not source.is_file():
        raise FileNotFoundError(source)

    staging = Path(__import__("tempfile").mkdtemp(prefix="simplicio-preview-"))
    staged = staging / source.name
    shutil.copyfile(source, staged)
    server = ThreadingHTTPServer(
        ("127.0.0.1", 0), partial(_quiet_handler, directory=str(staging))
    )
    server.daemon_threads = True
    thread = threading.Thread(target=server.serve_forever, name="simplicio-preview", daemon=True)
    thread.start()
    host, port = server.server_address
    target = PreviewTarget(source=source, staged=staged, url=f"http://{host}:{port}/{source.name}")
    try:
        yield target
    finally:
        server.shutdown()
        server.server_close()
        thread.join(timeout=2)
        shutil.rmtree(staging, ignore_errors=False)


@contextmanager
def serve_target(target: str | Path) -> Iterator[PreviewTarget]:
    """Convert a local ``file://`` target to a bounded HTTP preview."""

    value = str(target)
    path = _file_uri_path(value) if urlparse(value).scheme.lower() == "file" else Path(value)
    with serve_file(path) as preview:
        yield preview


def browser_result(browser_available: bool) -> dict[str, str]:
    """Return honest browser evidence without pretending to run a browser."""

    if not browser_available:
        return {"status": "NOT_EXECUTED", "reason": "browser_unavailable"}
    return {"status": "UNVERIFIED", "reason": "external_browser_evidence_required"}


def _main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("path", type=Path, help="one generated HTML/report file")
    parser.add_argument("--duration", type=float, default=30.0, help="seconds to keep the preview alive")
    args = parser.parse_args()
    if args.duration < 0:
        parser.error("--duration must be non-negative")
    with serve_file(args.path) as preview:
        print(
            json.dumps(
                {
                    "schema": SCHEMA,
                    "status": "READY",
                    "source": preview.source.name,
                    "url": preview.url,
                    "browser": browser_result(False),
                    "duration_seconds": args.duration,
                },
                sort_keys=True,
            ),
            flush=True,
        )
        if args.duration:
            time.sleep(args.duration)
    return 0


if __name__ == "__main__":
    raise SystemExit(_main())
