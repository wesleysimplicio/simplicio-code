from scripts.validate_component_release import COMPONENTS, SCHEMA, validate


def _manifest():
    return {
        "schema": SCHEMA,
        "compatibility": {"code_protocol": "CoordinatorProtocol/v1"},
        "components": [
            {"name": name, "version": "0.3.0", "commit": "a" * 40, "artifact_digest": "b" * 64, "protocol": f"{name}/v1"}
            for name in sorted(COMPONENTS)
        ],
    }


def test_pinned_bundle_is_ready_and_digest_is_stable():
    result = validate(_manifest())
    assert result["status"] == "ready"
    assert len(result["manifest_digest"]) == 64


def test_latest_is_rejected():
    manifest = _manifest()
    manifest["components"][0]["version"] = "latest"
    result = validate(manifest)
    assert result["status"] == "blocked"
    assert any("pinned version" in error for error in result["errors"])


def test_missing_component_and_digest_are_rejected():
    manifest = _manifest()
    manifest["components"] = manifest["components"][:-1]
    manifest["components"][0]["artifact_digest"] = "not-a-digest"
    result = validate(manifest)
    assert result["status"] == "blocked"
    assert any("missing components" in error for error in result["errors"])
    assert any("sha256 artifact_digest" in error for error in result["errors"])


def test_protocol_compatibility_is_required():
    manifest = _manifest()
    del manifest["compatibility"]
    result = validate(manifest)
    assert result["status"] == "blocked"
