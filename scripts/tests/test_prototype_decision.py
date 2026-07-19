from scripts.validate_prototype_decision import SCHEMA, validate


def _receipt(**overrides):
    value = {
        "schema": SCHEMA,
        "plan_id": "plan-1",
        "source_revision": "source-1",
        "validated_source_revision": "source-1",
        "decision_id": "decision-1",
        "decision": "accept",
        "assumptions": ["uses existing API"],
        "limitations": ["preview is not production code"],
        "artifacts": [{"id": "wire-1", "type": "wireframe", "title": "Home", "summary": "Main flow", "uri": "artifact://wire-1"}],
    }
    value.update(overrides)
    return value


def test_current_accept_authorizes_build():
    result = validate(_receipt(), build_requested=True)
    assert result["status"] == "ready"
    assert result["build_authorized"] is True


def test_revise_does_not_authorize_build():
    result = validate(_receipt(decision="revise"), build_requested=True)
    assert result["status"] == "blocked"
    assert result["build_authorized"] is False


def test_source_drift_invalidates_accept():
    result = validate(_receipt(validated_source_revision="source-2"), build_requested=True)
    assert result["status"] == "blocked"
    assert any("source drift" in error for error in result["errors"])


def test_malicious_artifact_uri_is_blocked():
    artifact = {"id": "bad", "type": "diagram", "title": "x", "summary": "y", "uri": "artifact://../secret"}
    result = validate(_receipt(artifacts=[artifact]))
    assert result["status"] == "blocked"
    assert any("sandbox" in error for error in result["errors"])
