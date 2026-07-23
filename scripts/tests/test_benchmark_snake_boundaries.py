from __future__ import annotations

import struct
from pathlib import Path

import sys

ROOT = Path(__file__).parents[2]
sys.path.insert(0, str(ROOT / "scripts"))

from benchmark_snake import receipt_payload  # noqa: E402
from hbp_receipt import _content_hash, write_ledger_atomic  # noqa: E402


def _records(data: bytes) -> list[tuple[int, str, str, str, str]]:
    assert data[:8] == b"HBP1\x01\x00\x00\x00"
    cursor = 8
    result = []
    while cursor < len(data):
        length = struct.unpack_from("<I", data, cursor)[0]
        cursor += 4
        body = data[cursor:cursor + length]
        cursor += length
        offset = 16
        fields = []
        for _ in range(3):
            size = struct.unpack_from("<I", body, offset)[0]
            offset += 4
            fields.append(body[offset:offset + size].decode())
            offset += size
        marker = body[offset]
        assert marker == 0
        offset += 1
        for _ in range(2):
            size = struct.unpack_from("<I", body, offset)[0]
            offset += 4
            fields.append(body[offset:offset + size].decode())
            offset += size
        assert offset == len(body)
        sequence = struct.unpack_from("<Q", body, 0)[0]
        result.append((sequence, fields[0], fields[1], fields[3], fields[4]))
    return result


def test_benchmark_receipts_are_hbp_and_hash_chained(tmp_path: Path) -> None:
    path = tmp_path / "events.hbp"
    write_ledger_atomic(path, [receipt_payload({"status": "PASS"}), receipt_payload({"status": "UNVERIFIED"})])
    records = _records(path.read_bytes())
    assert [record[0] for record in records] == [0, 1]
    assert records[0][1] == "code.record"
    assert bytes.fromhex(records[0][2]).decode().endswith("status=PASS\n")
    assert records[0][3] == "genesis"
    assert records[0][4] == _content_hash(0, records[0][3], records[0][2])
    assert records[1][3] == records[0][4]
    assert records[1][4] == _content_hash(1, records[1][3], records[1][2])
