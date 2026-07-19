from scripts.validate_loop_hub_client import SCHEMA, validate


def _status(**overrides):
    value = {
        "schema": SCHEMA,
        "mode": "auto",
        "hub": {"state": "ready", "endpoint": "local://loop-hub"},
        "services": [
            {"name": "runtime", "owner": "loop-hub"},
            {"name": "mapper", "owner": "loop-hub"},
            {"name": "scheduler", "owner": "loop-hub"},
        ],
    }
    value.update(overrides)
    return value


def test_auto_attaches_to_ready_hub():
    result = validate(_status())
    assert result["status"] == "ready"
    assert result["effective_mode"] == "hub"


def test_hub_mode_rejects_local_duplicate_runtime():
    status = _status(services=_status()["services"] + [{"name": "runtime", "owner": "code-process"}])
    result = validate(status)
    assert result["status"] == "blocked"
    assert any("reused from Loop Hub" in error for error in result["errors"])


def test_required_mode_blocks_when_hub_is_missing():
    result = validate(_status(mode="required", hub={"state": "missing"}, services=[]))
    assert result["status"] == "blocked"


def test_standalone_is_explicit_and_does_not_attach():
    result = validate(_status(mode="standalone", hub={"state": "missing"}, services=[]))
    assert result["status"] == "ready"
