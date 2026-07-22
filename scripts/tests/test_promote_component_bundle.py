import hashlib
import json
from pathlib import Path

import pytest

from scripts.release.promote_component_bundle import PromotionRejected, promote, rollback
from scripts.release.generate_component_client import render

ROOT = Path(__file__).parents[2]


def bundle(tmp_path: Path, version: str):
    artifacts = tmp_path / f"artifacts-{version}"
    artifacts.mkdir()
    components = []
    for name in ("agent-contracts", "code", "loop-hub", "runtime"):
        data = f"{version}-{name}".encode()
        (artifacts / name).write_bytes(data)
        component = {"name": name, "version": version, "commit": "a" * 40,
                     "protocol": f"{name}/v1", "artifact_digest": hashlib.sha256(data).hexdigest()}
        if name == "runtime":
            component["generated_client_digest"] = hashlib.sha256(render(ROOT / "docs/contracts/component-release-v1.schema.json").encode()).hexdigest()
        components.append(component)
    return {"schema": "simplicio.component-release/v1", "bundle_version": version,
            "components": components, "compatibility": {"code_protocol": "CoordinatorProtocol/v1",
            "protocol_ranges": {"runtime": {"min": 1, "max": 1}}}}, artifacts


def active_version(slots: Path):
    return json.loads((slots / "active/manifest.json").read_text())["bundle_version"]


def test_promotes_as_bundle_and_rolls_back(tmp_path):
    slots = tmp_path / "slots"
    first, first_artifacts = bundle(tmp_path, "1.0.0")
    assert promote(first, first_artifacts, slots, lambda _: True)["rollback_available"] is False
    second, second_artifacts = bundle(tmp_path, "1.1.0")
    assert promote(second, second_artifacts, slots, lambda _: True)["rollback_available"] is True
    assert active_version(slots) == "1.1.0"
    assert rollback(slots)["decision"] == "rolled_back"
    assert active_version(slots) == "1.0.0"


@pytest.mark.parametrize("failure", ("canary", "digest", "lock", "canary_tamper"))
def test_failure_never_changes_active_slot(tmp_path, failure):
    slots = tmp_path / "slots"
    first, artifacts = bundle(tmp_path, "1.0.0")
    promote(first, artifacts, slots, lambda _: True)
    candidate, candidate_artifacts = bundle(tmp_path, "1.1.0")
    if failure == "digest":
        (candidate_artifacts / "runtime").write_bytes(b"tampered")
    if failure == "lock":
        (slots / ".promotion.lock").write_text("busy")
    def canary(stage):
        if failure == "canary_tamper":
            (stage / "runtime").chmod(0o700)
            (stage / "runtime").write_bytes(b"tampered by canary")
        return failure != "canary"
    with pytest.raises(PromotionRejected):
        promote(candidate, candidate_artifacts, slots, canary)
    assert active_version(slots) == "1.0.0"
