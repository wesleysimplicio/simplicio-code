import importlib.util
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
SPEC = importlib.util.spec_from_file_location("benchmark_snake", ROOT / "scripts/benchmark_snake.py")
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader
SPEC.loader.exec_module(MODULE)


def test_events_happy_path_is_idempotent_and_has_no_partial_file():
    events = [
        {"event": "agent_started", "agent": "simplicio", "argv": ["code", "a b"], "ts_ms": 1},
        {"event": "agent_finished", "agent": "simplicio", "status": "PASS", "wall_ms": 2, "ts_ms": 3},
    ]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "events.hbp"
        MODULE.write_events(path, events)
        first = path.read_bytes()
        MODULE.write_events(path, events)
        assert path.read_bytes() == first
        assert not (path.parent / ".events.hbp.tmp").exists()
        assert len(MODULE.decode_records(first)) == 2


def test_events_corruption_fails_closed_and_preserves_published_file():
    events = [{"event": "agent_started", "agent": "simplicio", "argv": [], "ts_ms": 1}]
    with tempfile.TemporaryDirectory() as tmp:
        path = Path(tmp) / "events.hbp"
        MODULE.write_events(path, events)
        published = path.read_bytes()
        corrupt = bytearray(published)
        corrupt[-1] ^= 1
        try:
            MODULE.decode_records(bytes(corrupt))
        except ValueError:
            pass
        else:
            raise AssertionError("corrupt HBP must fail")
        try:
            MODULE.write_events(path, [{"agent": "missing-event"}])
        except ValueError:
            pass
        else:
            raise AssertionError("invalid event must fail")
        assert path.read_bytes() == published
