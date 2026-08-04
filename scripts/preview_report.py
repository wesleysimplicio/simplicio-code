#!/usr/bin/env python3
"""Serve one local report directory over loopback for browser verification."""
from __future__ import annotations

import argparse
import functools
import http.server
import socketserver
from pathlib import Path
from urllib.parse import urlparse


def validate_url(url: str) -> None:
    if urlparse(url).scheme.lower() == "file":
        raise ValueError("file:// is not supported; serve the report with this helper")
    if urlparse(url).scheme and urlparse(url).scheme.lower() not in {"http", "https"}:
        raise ValueError("only http:// and https:// URLs are supported")


def serve(directory: Path, host: str = "127.0.0.1", port: int = 0) -> str:
    directory = directory.resolve()
    if not directory.is_dir():
        raise ValueError(f"report directory does not exist: {directory}")
    handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=str(directory))
    with socketserver.TCPServer((host, port), handler) as server:
        actual_port = server.server_address[1]
        print(f"http://{host}:{actual_port}/", flush=True)
        try:
            server.serve_forever()
        except KeyboardInterrupt:
            pass
    return f"http://{host}:{actual_port}/"


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("directory", type=Path)
    parser.add_argument("--url", help="validate a browser URL before opening it")
    parser.add_argument("--port", type=int, default=0)
    args = parser.parse_args(argv)
    if args.url:
        try:
            validate_url(args.url)
        except ValueError as exc:
            parser.error(str(exc))
        print(args.url)
        return 0
    serve(args.directory, port=args.port)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
