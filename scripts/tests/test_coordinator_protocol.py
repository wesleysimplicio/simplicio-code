from scripts.validate_coordinator_protocol import SCHEMA, validate


def _envelope(**overrides):
    value = {
        "schema": SCHEMA,
        "coordinator": "simplicio-agent",
        "workspace_id": "ws-1",
        "session_id": "session-1",
        "turn_id": "turn-1",
        "policy_revision": "policy-1",
        "events": [
            {"type": "session.open", "sequence": 1, "causal_id": "c1"},
            {"type": "turn.start", "sequence": 2, "causal_id": "c2"},
            {"type": "turn.cancel", "sequence": 3, "causal_id": "c3"},
            {"type": "turn.resume", "sequence": 4, "causal_id": "c4"},
        ],
    }
    value.update(overrides)
    return value


def test_valid_agent_host_sequence_is_ready():
    result = validate(_envelope())
    assert result["status"] == "ready"
    assert result["state"] == "running"


def test_missing_identity_blocks_fail_closed():
    value = _envelope(session_id="")
    result = validate(value)
    assert result["status"] == "blocked"
    assert "session_id is required" in result["errors"]


def test_unknown_coordinator_and_event_block():
    value = _envelope(coordinator="other", events=[{"type": "tool.execute", "sequence": 1, "causal_id": "c"}])
    result = validate(value)
    assert result["status"] == "blocked"
    assert any("unknown type" in error for error in result["errors"])


def test_duplicate_sequences_block():
    value = _envelope(events=[{"type": "session.open", "sequence": 1, "causal_id": "c1"}, {"type": "turn.start", "sequence": 1, "causal_id": "c2"}])
    result = validate(value)
    assert result["status"] == "blocked"
    assert any("strictly increasing" in error for error in result["errors"])


def test_builtin_is_diagnostic_only():
    value = _envelope(coordinator="builtin", mode="diagnostic")
    assert validate(value)["status"] == "ready"


def test_builtin_productive_turn_blocks():
    value = _envelope(coordinator="builtin")
    result = validate(value)
    assert result["status"] == "blocked"
    assert any("productive turns require" in error for error in result["errors"])
