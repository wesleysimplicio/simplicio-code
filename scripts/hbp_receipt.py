"""Minimal Runtime HBP v1 writer for Code-owned Python receipts.

The layout mirrors ``simplicio-code-formats``.  Payload encoding is deliberately
separate: callers provide opaque bytes and cannot silently fall back to JSON.
"""
from __future__ import annotations

import hashlib
import os
import struct
from pathlib import Path

MAGIC = b"HBP1\x01\x00\x00\x00"
TOPIC = "code.record"
PROVENANCE = "simplicio-code"
GENESIS = "genesis"
MAX_FIELD_BYTES = 4 * 1024 * 1024
MAX_RECORD_BYTES = 16 * 1024 * 1024


def _field(value: str) -> bytes:
    encoded = value.encode("utf-8")
    if len(encoded) > MAX_FIELD_BYTES:
        raise ValueError("HBP field exceeds the safety limit")
    return struct.pack("<I", len(encoded)) + encoded


def _content_hash(sequence: int, previous: str, payload: str) -> str:
    digest = hashlib.sha256()
    for value in (str(sequence), previous, TOPIC, payload, PROVENANCE, ""):
        encoded = value.encode("utf-8")
        digest.update(struct.pack("<Q", len(encoded)))
        digest.update(encoded)
    return digest.hexdigest()


def _encode_record(sequence: int, previous: str, payload: bytes) -> tuple[bytes, str]:
    """Encode one HBP record and return its hash for the next chain link."""
    payload_hex = payload.hex()
    content_hash = _content_hash(sequence, previous, payload_hex)
    body = (
        struct.pack("<QQ", sequence, 0)
        + _field(TOPIC)
        + _field(payload_hex)
        + _field(PROVENANCE)
        + b"\x00"
        + _field(previous)
        + _field(content_hash)
    )
    if len(body) > MAX_RECORD_BYTES:
        raise ValueError("HBP record exceeds the safety limit")
    return struct.pack("<I", len(body)) + body, content_hash


def encode_records(payloads: list[bytes]) -> bytes:
    """Encode a bounded genesis-linked HBP v1 ledger."""
    output = bytearray(MAGIC)
    previous = GENESIS
    for sequence, payload in enumerate(payloads):
        record, previous = _encode_record(sequence, previous, payload)
        output.extend(record)
    return bytes(output)


def encode_record(payload: bytes) -> bytes:
    """Encode one genesis-linked ``code.record`` in Runtime's HBP v1 layout."""
    return encode_records([payload])


def write_ledger_atomic(path: Path, payloads: list[bytes]) -> None:
    """Publish a complete HBP ledger without exposing a partial file."""
    encoded = encode_records(payloads)
    path.parent.mkdir(parents=True, exist_ok=True)
    temporary = path.with_name(f".{path.name}.tmp")
    with temporary.open("wb") as stream:
        stream.write(encoded)
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)


def write_atomic(path: Path, payload: bytes) -> None:
    """Publish a single-record receipt without exposing a partial ledger."""
    write_ledger_atomic(path, [payload])
