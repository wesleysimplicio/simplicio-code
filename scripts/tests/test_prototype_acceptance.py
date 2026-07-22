import copy
import json

from scripts.validate_prototype_acceptance import benchmark, main, validate


def receipts():
    common = {"source_revision": "abc123", "plan_id": "plan-1"}
    loop = {**common, "schema": "simplicio.loop-prototype-capabilities/v1", "accepted": True,
            "states": ["prototype_required", "gallery", "compare", "revise", "reject", "accept", "stale", "build_authorized"]}
    runtime = {**common, "schema": "simplicio.runtime-prototype-preflight/v1", "negotiated": True,
               "binary_version": "1.2.3", "binary_sha256": "a" * 64,
               "tools": ["simplicio_prototype_artifact_write"]}
    steps = ["install", "prototype", "compare", "reject", "revise", "accept", "build", "delivery"]
    e2e = {**common, "schema": "simplicio.prototype-product-e2e/v1", "failure_injection_passed": True,
           "replay_hash_match": True, "runs": [{"surface": s, "status": "passed", "steps": steps} for s in ("tui", "ui", "headless", "acp")]}
    return loop, runtime, e2e


def test_complete_real_evidence_is_ready_and_deterministic():
    loop, runtime, e2e = receipts()
    first = validate(loop, runtime, e2e)
    assert first["status"] == "ready"
    assert first == validate(loop, runtime, e2e)
    assert first["metrics"]["validation_duration_ms"] is None


def test_missing_runtime_and_loop_capabilities_fail_closed():
    loop, runtime, e2e = receipts()
    loop["states"].remove("stale")
    runtime.update(negotiated=False, tools=[], binary_sha256="not-a-digest")
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert any("stale" in error for error in result["errors"])
    assert any("real binary" in error for error in result["errors"])
    assert any("missing tools" in error for error in result["errors"])


def test_revision_drift_and_incomplete_surface_fail_closed():
    loop, runtime, e2e = receipts()
    runtime["source_revision"] = "stale"
    e2e["runs"][0]["steps"].remove("delivery")
    e2e["failure_injection_passed"] = False
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert any("identical" in error for error in result["errors"])
    assert any("tui" in error for error in result["errors"])
    assert any("rollback" in error for error in result["errors"])


def test_input_receipts_are_not_mutated_or_embedded():
    loop, runtime, e2e = receipts()
    loop["private"] = "do-not-copy"
    before = copy.deepcopy((loop, runtime, e2e))
    result = validate(loop, runtime, e2e)
    assert (loop, runtime, e2e) == before
    assert "do-not-copy" not in str(result)


def test_every_receipt_schema_identity_and_replay_gate_is_enforced():
    loop, runtime, e2e = receipts()
    loop.update(schema="wrong", accepted=False, plan_id="other")
    runtime.update(schema="wrong", binary_version="", plan_id="runtime-plan")
    e2e.update(schema="wrong", replay_hash_match=False)
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert len(result["errors"]) >= 7


def test_non_list_contract_collections_fail_closed():
    loop, runtime, e2e = receipts()
    loop["states"] = "all"
    runtime["tools"] = "all"
    e2e["runs"] = {"surface": "all"}
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert any("missing states" in error for error in result["errors"])


def test_malformed_nested_collections_fail_closed_instead_of_crashing():
    loop, runtime, e2e = receipts()
    loop["states"] = [{"forged": "state"}]
    runtime["tools"] = [None]
    e2e["runs"][0]["steps"] = [{"forged": "step"}]
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert any("array of non-empty strings" in error for error in result["errors"])


def test_duplicate_or_contradictory_surface_cannot_turn_failure_into_success():
    loop, runtime, e2e = receipts()
    failed = copy.deepcopy(e2e["runs"][0])
    failed["status"] = "failed"
    e2e["runs"].insert(0, failed)
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert any("contradictory or duplicate" in error for error in result["errors"])


def test_unknown_surface_and_non_finite_json_fail_closed():
    loop, runtime, e2e = receipts()
    e2e["runs"][0]["surface"] = "web"
    loop["measurement"] = float("nan")
    result = validate(loop, runtime, e2e)
    assert result["status"] == "blocked"
    assert any("unknown surface" in error for error in result["errors"])
    assert any("canonical JSON" in error for error in result["errors"])


def test_benchmark_reports_measured_numbers():
    loop, runtime, e2e = receipts()
    result = benchmark(loop, runtime, e2e, 10)
    assert result["iterations"] == 10
    assert result["elapsed_ms"] >= 0
    assert result["mean_us"] >= 0


def test_cli_ready_and_invalid_input(tmp_path, monkeypatch, capsys):
    paths = []
    for name, data in zip(("loop", "runtime", "e2e"), receipts()):
        path = tmp_path / f"{name}.json"
        path.write_text(json.dumps(data), encoding="utf-8")
        paths.append(path)
    monkeypatch.setattr("sys.argv", ["validator", "--loop", str(paths[0]), "--runtime", str(paths[1]), "--e2e", str(paths[2]), "--benchmark", "2"])
    assert main() == 0
    assert json.loads(capsys.readouterr().out)["benchmark"]["iterations"] == 2

    paths[0].write_text("[]", encoding="utf-8")
    monkeypatch.setattr("sys.argv", ["validator", "--loop", str(paths[0]), "--runtime", str(paths[1]), "--e2e", str(paths[2])])
    assert main() == 2
    assert json.loads(capsys.readouterr().out)["status"] == "blocked"
