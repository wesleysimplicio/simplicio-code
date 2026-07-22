from scripts.validate_coordinator_protocol import (
    COGNITIVE_AUTHORITY,
    EFFECT_AUTHORITY,
    SCHEMA,
    validate,
)


def _envelope(**overrides):
    value = {
        "schema": SCHEMA,
        "coordinator": "external",
        "cognitive_authority": COGNITIVE_AUTHORITY,
        "effect_authority": EFFECT_AUTHORITY,
        "provider_activation": "none",
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


def test_valid_external_invoker_sequence_is_ready():
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


def test_internal_coordinators_block_productive_turns():
    value = _envelope(coordinator="builtin")
    result = validate(value)
    assert result["status"] == "blocked"
    assert any("external invoking LLM" in error for error in result["errors"])

    value = _envelope(coordinator="simplicio-agent")
    result = validate(value)
    assert result["status"] == "blocked"
    assert any("external invoking LLM" in error for error in result["errors"])


def test_productive_turn_rejects_internal_provider_or_local_llm_activation():
    for activation in ("internal-provider", "local-llm", "fallback"):
        result = validate(_envelope(provider_activation=activation))
        assert result["status"] == "blocked"
        assert "provider_activation must be none" in result["errors"]


def test_authority_boundaries_are_required_and_fail_closed():
    result = validate(
        _envelope(cognitive_authority="simplicio-agent", effect_authority="code")
    )
    assert result["status"] == "blocked"
    assert "cognitive_authority must be external-invoker" in result["errors"]
    assert "effect_authority must be simplicio-runtime" in result["errors"]
