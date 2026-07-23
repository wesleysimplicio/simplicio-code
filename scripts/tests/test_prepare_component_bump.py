import base64
import hashlib
import json
from pathlib import Path
import subprocess

import pytest

from scripts.release.generate_component_client import render
from scripts.release.prepare_component_bump import BumpRejected, canonical, main, prepare

ROOT = Path(__file__).parents[2]
SCHEMA = ROOT / "docs/contracts/component-release-v1.schema.json"


def signed_event(tmp_path: Path):
    trust = tmp_path / "trust"
    artifacts = tmp_path / "artifacts"
    trust.mkdir(); artifacts.mkdir()
    subprocess.run(["openssl", "genpkey", "-algorithm", "ED25519", "-out", str(tmp_path / "private.pem")], check=True)
    subprocess.run(["openssl", "pkey", "-in", str(tmp_path / "private.pem"), "-pubout", "-out", str(trust / "publisher.pem")], check=True)
    components = []
    for name in ("agent-contracts", "code", "loop-hub", "runtime"):
        content = f"immutable-{name}".encode()
        (artifacts / name).write_bytes(content)
        components.append({"name": name, "version": "1.2.3", "commit": "a" * 40,
                           "artifact_digest": hashlib.sha256(content).hexdigest(), "protocol": f"{name}/v1",
                           **({"generated_client_digest": hashlib.sha256(render(SCHEMA).encode()).hexdigest()}
                              if name == "runtime" else {})})
    manifest = {"schema": "simplicio.component-release/v1", "bundle_version": "1.2.3",
                "components": components, "compatibility": {"code_protocol": "CoordinatorProtocol/v1",
                "protocol_ranges": {name: {"min": 1, "max": 2} for name in ("agent-contracts", "code", "loop-hub", "runtime")}}}
    payload = {"schema": "simplicio.release-event/v1", "event_id": "release-123", "producer": "release-bot",
               "sequence": 7, "manifest": manifest, "bundle_digest": hashlib.sha256(canonical(manifest)).hexdigest()}
    payload_file = tmp_path / "payload"; signature_file = tmp_path / "signature"
    payload_file.write_bytes(canonical(payload))
    subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(tmp_path / "private.pem"), "-rawin",
                    "-in", str(payload_file), "-out", str(signature_file)], check=True)
    return {"key_id": "publisher", "signature": base64.b64encode(signature_file.read_bytes()).decode(), "payload": payload}, trust, artifacts


def test_verified_event_prepares_deterministic_bump_and_deduplicates(tmp_path):
    event, trust, artifacts = signed_event(tmp_path)
    manifest, state, receipt = prepare(event, trust, artifacts, SCHEMA)
    assert canonical(manifest) == canonical(event["payload"]["manifest"])
    assert state["events"][0]["bundle_digest"] == event["payload"]["bundle_digest"]
    assert receipt["artifact_digests"]["runtime"] == event["payload"]["manifest"]["components"][-1]["artifact_digest"]
    duplicate_manifest, duplicate_state, duplicate_receipt = prepare(event, trust, artifacts, SCHEMA, state)
    assert (duplicate_manifest, duplicate_state) == (manifest, state)
    assert duplicate_receipt["duplicate"] is True


@pytest.mark.parametrize("failure", ["signature", "digest", "missing", "revoked", "stale"])
def test_release_inputs_fail_closed_with_next_action(tmp_path, failure):
    event, trust, artifacts = signed_event(tmp_path)
    state = None
    if failure == "signature": event["signature"] = base64.b64encode(b"bad").decode()
    elif failure == "digest": (artifacts / "runtime").write_bytes(b"tampered")
    elif failure == "missing": (artifacts / "loop-hub").unlink()
    elif failure == "revoked": (trust / "publisher.pem").unlink()
    else: state = {"schema": "simplicio.release-bump-state/v1", "events": [
        {"event_id": "old", "producer": "release-bot", "sequence": 8, "bundle_digest": "0" * 64}]}
    with pytest.raises(BumpRejected) as rejected:
        prepare(event, trust, artifacts, SCHEMA, state)
    assert any(word in str(rejected.value) for word in ("signature", "digest", "missing", "revoked", "stale"))


def test_incompatible_protocol_emits_migration_action(tmp_path):
    event, trust, artifacts = signed_event(tmp_path)
    event["payload"]["manifest"]["compatibility"]["protocol_ranges"]["runtime"] = {"min": 2, "max": 3}
    # Re-sign the intentionally incompatible but otherwise well-formed payload.
    event["payload"]["bundle_digest"] = hashlib.sha256(canonical(event["payload"]["manifest"])).hexdigest()
    payload = tmp_path / "incompatible"; signature = tmp_path / "incompatible.sig"
    payload.write_bytes(canonical(event["payload"]))
    subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(tmp_path / "private.pem"), "-rawin",
                    "-in", str(payload), "-out", str(signature)], check=True)
    event["signature"] = base64.b64encode(signature.read_bytes()).decode()
    with pytest.raises(BumpRejected, match="migration event"):
        prepare(event, trust, artifacts, SCHEMA)


def test_generated_client_digest_must_match_reproducible_bindings(tmp_path):
    event, trust, artifacts = signed_event(tmp_path)
    event["payload"]["manifest"]["components"][-1]["generated_client_digest"] = "0" * 64
    event["payload"]["bundle_digest"] = hashlib.sha256(canonical(event["payload"]["manifest"])).hexdigest()
    payload = tmp_path / "wrong-client"; signature = tmp_path / "wrong-client.sig"
    payload.write_bytes(canonical(event["payload"]))
    subprocess.run(["openssl", "pkeyutl", "-sign", "-inkey", str(tmp_path / "private.pem"), "-rawin",
                    "-in", str(payload), "-out", str(signature)], check=True)
    event["signature"] = base64.b64encode(signature.read_bytes()).decode()
    with pytest.raises(BumpRejected, match="generated_client_digest"):
        prepare(event, trust, artifacts, SCHEMA)


def test_cli_writes_canonical_outputs_and_reports_duplicate(tmp_path, monkeypatch, capsys):
    event, trust, artifacts = signed_event(tmp_path)
    event_path, manifest_path, state_path = tmp_path / "event.json", tmp_path / "out/manifest.json", tmp_path / "state.json"
    event_path.write_text(json.dumps(event))
    receipt_path = tmp_path / "receipt.json"
    argv = ["prepare", "--event", str(event_path), "--trust-dir", str(trust),
            "--artifacts-dir", str(artifacts), "--manifest-out", str(manifest_path), "--state", str(state_path),
            "--receipt-out", str(receipt_path)]
    monkeypatch.setattr("sys.argv", argv)
    assert main() == 0
    assert json.loads(manifest_path.read_text()) == event["payload"]["manifest"]
    assert json.loads(receipt_path.read_text())["decision"] == "verified"
    assert main() == 0
    assert '"status": "ready"' in capsys.readouterr().out


@pytest.mark.parametrize("mutation, message", [
    (lambda event: event.update(extra=True), "envelope"),
    (lambda event: event.update(key_id="../key"), "key_id"),
    (lambda event: event["payload"].update(extra=True), "payload"),
    (lambda event: event["payload"].update(schema="v2"), "schema"),
    (lambda event: event["payload"].update(event_id="bad id"), "event_id"),
    (lambda event: event["payload"].update(sequence=True), "sequence"),
])
def test_malformed_envelopes_are_rejected_before_promotion(tmp_path, mutation, message):
    event, trust, artifacts = signed_event(tmp_path)
    mutation(event)
    with pytest.raises(BumpRejected, match=message):
        prepare(event, trust, artifacts, SCHEMA)


def test_conflict_and_invalid_history_fail_closed(tmp_path):
    event, trust, artifacts = signed_event(tmp_path)
    bad_state = {"schema": "wrong", "events": []}
    with pytest.raises(BumpRejected, match="history"):
        prepare(event, trust, artifacts, SCHEMA, bad_state)
    conflict = {"schema": "simplicio.release-bump-state/v1", "events": [{
        "event_id": "release-123", "producer": "other", "sequence": 1, "bundle_digest": "0" * 64}]}
    with pytest.raises(BumpRejected, match="conflicts"):
        prepare(event, trust, artifacts, SCHEMA, conflict)


def test_cli_reports_bad_json_as_blocked(tmp_path, monkeypatch, capsys):
    event_path = tmp_path / "bad.json"; event_path.write_text("{")
    monkeypatch.setattr("sys.argv", ["prepare", "--event", str(event_path), "--trust-dir", str(tmp_path),
        "--artifacts-dir", str(tmp_path), "--manifest-out", str(tmp_path / "manifest"), "--state", str(tmp_path / "state"),
        "--receipt-out", str(tmp_path / "receipt")])
    assert main() == 2
    assert '"status": "blocked"' in capsys.readouterr().out
