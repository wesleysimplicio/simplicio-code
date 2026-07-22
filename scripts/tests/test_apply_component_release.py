import hashlib
import json
from pathlib import Path

import pytest

from scripts.release.apply_component_release import canonical_digest, main, prepare
from scripts.release.generate_component_client import render

ROOT = Path(__file__).parents[2]
SCHEMA = ROOT / "docs/contracts/component-release-v1.schema.json"


def event() -> dict:
    generated_digest = hashlib.sha256(render(SCHEMA).encode()).hexdigest()
    manifest = {
        "schema": "simplicio.component-release/v1",
        "bundle_version": "0.3.1",
        "components": [
            {"name": name, "version": "1.2.3", "commit": "a" * 40,
             "protocol": f"{name}/v1", "artifact_digest": "b" * 64,
             **({"generated_client_digest": generated_digest} if name == "runtime" else {})}
            for name in ("agent-contracts", "code", "loop-hub", "runtime")
        ],
        "compatibility": {"code_protocol": "CoordinatorProtocol/v1", "protocol_ranges": {"runtime": {"min": 1, "max": 1}}},
    }
    return {"schema": "simplicio.release-event/v1", "event_id": "runtime-42", "producer": "simplicio-runtime",
            "sequence": 42, "manifest": manifest, "bundle_digest": canonical_digest(manifest)}


def test_prepares_reproducible_lock_and_receipt(tmp_path):
    candidate = event()
    manifest, generated, receipt = prepare(candidate, SCHEMA)
    assert canonical_digest(manifest) == receipt["active_digest"]
    assert hashlib.sha256(generated.encode()).hexdigest() == receipt["generated_client_digest"]
    assert receipt["migration_required"] is False


@pytest.mark.parametrize("mutation, message", [
    (lambda value: value.update(bundle_digest="0" * 64), "bundle_digest"),
    (lambda value: value.update(producer="untrusted"), "producer"),
    (lambda value: value["manifest"]["components"][-1].update(generated_client_digest="0" * 64), "generated_client_digest"),
])
def test_fails_closed_on_unverified_input(mutation, message):
    candidate = event()
    mutation(candidate)
    if message == "generated_client_digest":
        candidate["bundle_digest"] = canonical_digest(candidate["manifest"])
    with pytest.raises(ValueError, match=message):
        prepare(candidate, SCHEMA)


def test_protocol_change_requests_migration(tmp_path):
    candidate = event()
    current = tmp_path / "component-release.json"
    old = json.loads(json.dumps(candidate["manifest"]))
    old["compatibility"]["code_protocol"] = "CoordinatorProtocol/v0"
    current.write_text(json.dumps(old), encoding="utf-8")
    assert prepare(candidate, SCHEMA, current)[2]["migration_required"] is True


def test_cli_writes_outputs_and_check_is_reproducible(tmp_path):
    event_path = tmp_path / "event.json"
    manifest = tmp_path / "release" / "component-release.json"
    generated = tmp_path / "generated.rs"
    receipt = tmp_path / "release" / "receipt.json"
    event_path.write_text(json.dumps(event()), encoding="utf-8")
    args = [str(event_path), "--schema", str(SCHEMA), "--manifest", str(manifest),
            "--generated", str(generated), "--receipt", str(receipt)]
    assert main(args) == 0
    assert json.loads(manifest.read_text())["bundle_version"] == "0.3.1"
    assert main([*args, "--check"]) == 0
    generated.write_text("manual fork", encoding="utf-8")
    with pytest.raises(SystemExit, match="stale release outputs"):
        main([*args, "--check"])


@pytest.mark.parametrize("change, message", [
    (lambda value: value.update(schema="wrong"), "schema"),
    (lambda value: value.update(sequence=0), "sequence"),
    (lambda value: value["manifest"].update(bundle_version="latest"), "invalid component release"),
])
def test_event_contract_rejections(change, message):
    candidate = event()
    change(candidate)
    with pytest.raises(ValueError, match=message):
        prepare(candidate, SCHEMA)
