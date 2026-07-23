from scripts.validate_prototype_decision import ARTIFACT_TYPES, SCHEMA, render, validate


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
        "provenance": ["runtime://map/repo"],
        "ac_coverage": ["AC-1"],
        "artifacts": [{"id": "wire-1", "type": "wireframe", "title": "Home", "summary": "Main flow", "uri": "artifact://wire-1", "source_revision": "source-1", "digest": "sha256:wire-1", "evidence": [{"id": "e1", "label": "test", "uri": "runtime://evidence/e1"}], "ac_coverage": ["AC-1"]}],
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


def test_encoded_and_windows_artifact_traversal_is_blocked():
    for uri in (
        "artifact://%2e%2e/secret",
        "runtime://prototype-first/%2E%2E/secret",
        "artifact://candidate\\..\\secret",
        "artifact://candidate/%00secret",
        "artifact://candidate/%zz",
        "https://example.invalid/artifact",
    ):
        artifact = _receipt()["artifacts"][0].copy()
        artifact["uri"] = uri
        result = validate(_receipt(artifacts=[artifact]))
        assert any("sandbox" in error for error in result["errors"]), uri


def test_comparison_is_recomputed_and_fail_closed():
    left = _receipt()["artifacts"][0]
    right = left.copy()
    right.update(id="wire-2", summary="Alternate flow")
    comparison = {
        "left_artifact_id": "wire-1",
        "right_artifact_id": "wire-2",
        "changed_fields": ["summary"],
    }
    assert validate(_receipt(artifacts=[left, right], comparison=comparison))["status"] == "ready"

    comparison["changed_fields"] = []
    result = validate(_receipt(artifacts=[left, right], comparison=comparison))
    assert any("changed_fields" in error for error in result["errors"])

    comparison.update(left_artifact_id="../wire-1", right_artifact_id="wire-1")
    result = validate(_receipt(artifacts=[left, right], comparison=comparison))
    assert any("unsafe artifact id" in error for error in result["errors"])

    for invalid, message in (
        ("not-an-object", "must be an object"),
        ({"left_artifact_id": "wire-1", "right_artifact_id": "wire-1"}, "distinct"),
        ({"left_artifact_id": "wire-1", "right_artifact_id": "missing"}, "unknown"),
    ):
        result = validate(_receipt(artifacts=[left, right], comparison=invalid))
        assert any(message in error for error in result["errors"])


def test_source_revision_argument_invalidates_receipt():
    result = validate(_receipt(), current_source_revision="source-2", build_requested=True)
    assert result["state"] == "stale"
    assert result["build_authorized"] is False


def test_all_text_surfaces_share_decision_state():
    receipt = _receipt()
    for surface in ("tui", "ui", "headless", "acp"):
        output = render(receipt, surface=surface, current_source_revision="source-1")
        assert "accept" in output
        assert "build_authorized" in output or "Build: AUTHORIZED" in output


def test_all_text_surfaces_expose_explicit_build_authorization():
    receipt = _receipt()
    for surface in ("tui", "ui", "headless", "acp"):
        output = render(
            receipt,
            surface=surface,
            current_source_revision="source-1",
            build_requested=True,
        )
        assert "AUTHORIZED" in output or '"build_authorized": true' in output


def test_missing_evidence_and_coverage_block_accept():
    artifact = _receipt()["artifacts"][0]
    artifact.pop("evidence")
    artifact.pop("ac_coverage")
    result = validate(_receipt(artifacts=[artifact]), build_requested=True)
    assert result["status"] == "blocked"
    assert any("evidence" in error for error in result["errors"])
    assert any("coverage" in error for error in result["errors"])


def test_adjacent_decision_wire_format_is_supported():
    assert validate(_receipt(decision={"type": "accept"}))['status'] == "ready"
    assert validate(_receipt(decision={"type": "revise", "data": {"feedback": "change"}}))['state'] == "revise_requested"
    assert validate(_receipt(decision={"type": "reject", "data": {"reason": "no"}}))['state'] == "rejected"


def test_all_prototype_artifact_types_are_accepted():
    for artifact_type in ARTIFACT_TYPES:
        artifact = _receipt()["artifacts"][0].copy()
        artifact["type"] = artifact_type
        assert validate(_receipt(artifacts=[artifact]))["status"] == "ready"
