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
MAX_RECORDS = 100_000
MAX_LEDGER_BYTES = 64 * 1024 * 1024


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
    if len(payloads) > MAX_RECORDS:
        raise ValueError("HBP ledger exceeds the record limit")
    output = bytearray(MAGIC)
    previous = GENESIS
    for sequence, payload in enumerate(payloads):
        record, previous = _encode_record(sequence, previous, payload)
        output.extend(record)
    return bytes(output)


def decode_records(data: bytes) -> list[bytes]:
    """Validate a Runtime HBP v1 ledger and return its opaque payloads."""
    if len(data) > MAX_LEDGER_BYTES or not data.startswith(MAGIC):
        raise ValueError("invalid HBP header or ledger size")
    offset = len(MAGIC)
    payloads: list[bytes] = []
    previous = GENESIS
    while offset < len(data):
        if len(payloads) >= MAX_RECORDS or offset + 4 > len(data):
            raise ValueError("invalid HBP record count or truncated length")
        length = struct.unpack_from("<I", data, offset)[0]
        offset += 4
        if length < 16 or length > MAX_RECORD_BYTES or offset + length > len(data):
            raise ValueError("invalid HBP record length")
        body = memoryview(data)[offset:offset + length]
        offset += length
        sequence, _timestamp = struct.unpack_from("<QQ", body, 0)
        if sequence != len(payloads):
            raise ValueError("non-contiguous HBP sequence")
        cursor = 16

        def field() -> str:
            nonlocal cursor
            if cursor + 4 > len(body):
                raise ValueError("truncated HBP field")
            size = struct.unpack_from("<I", body, cursor)[0]
            cursor += 4
            if size > MAX_FIELD_BYTES or cursor + size > len(body):
                raise ValueError("invalid HBP field length")
            try:
                value = bytes(body[cursor:cursor + size]).decode("utf-8")
            except UnicodeDecodeError as exc:
                raise ValueError("invalid HBP UTF-8 field") from exc
            cursor += size
            return value

        topic, payload_hex, provenance = field(), field(), field()
        if cursor >= len(body) or body[cursor] != 0:
            raise ValueError("invalid HBP optional marker")
        cursor += 1
        actual_previous, content_hash = field(), field()
        if cursor != len(body) or topic != TOPIC or provenance != PROVENANCE:
            raise ValueError("unsupported HBP record domain")
        try:
            payload = bytes.fromhex(payload_hex)
        except ValueError as exc:
            raise ValueError("invalid HBP payload") from exc
        if actual_previous != previous or content_hash != _content_hash(sequence, actual_previous, payload_hex):
            raise ValueError("HBP content hash mismatch")
        payloads.append(payload)
        previous = content_hash
    return payloads


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
