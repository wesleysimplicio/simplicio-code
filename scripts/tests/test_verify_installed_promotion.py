import copy
from pathlib import Path

import pytest

from scripts.release.promote_component_bundle import promote
from scripts.release.prepare_component_bump import canonical, prepare
from scripts.release.verify_installed_promotion import EvidenceRejected, verify
from scripts.tests.test_prepare_component_bump import SCHEMA, signed_event


def installed_fixture(tmp_path: Path):
    # Locally signed synthetic data exercises the boundary; it is intentionally
    # not represented as evidence that an external publisher exists.
    event, trust, artifacts = signed_event(tmp_path)
    manifest, _, bump_receipt = prepare(event, trust, artifacts, SCHEMA)
    slots = tmp_path / "slots"
    promotion_receipt = promote(manifest, artifacts, slots, lambda _slot: True)
    return event, trust, artifacts, slots, bump_receipt, promotion_receipt


def test_signed_event_replay_proves_installed_promotion_deterministically(tmp_path):
    inputs = installed_fixture(tmp_path)
    event, trust, artifacts, slots, bump_receipt, promotion_receipt = inputs
    first = verify(event, trust, artifacts, SCHEMA, slots / "active", bump_receipt, promotion_receipt)
    second = verify(event, trust, artifacts, SCHEMA, slots / "active", bump_receipt, promotion_receipt)

    assert canonical(first) == canonical(second)
    assert first["decision"] == "verified-installed"
    assert first["bundle_digest"] == promotion_receipt["active_digest"]
    assert first["installed_artifact_digests"] == bump_receipt["artifact_digests"]


@pytest.mark.parametrize("broken_link", ["installed", "bump", "promotion"])
def test_provenance_chain_fails_closed(tmp_path, broken_link):
    event, trust, artifacts, slots, bump_receipt, promotion_receipt = installed_fixture(tmp_path)
    bump_receipt = copy.deepcopy(bump_receipt)
    promotion_receipt = copy.deepcopy(promotion_receipt)
    if broken_link == "installed":
        (slots / "active" / "runtime").write_bytes(b"modified after promotion")
    elif broken_link == "bump":
        bump_receipt["signing_key_id"] = "unrelated-key"
    else:
        promotion_receipt["active_digest"] = "0" * 64

    with pytest.raises(EvidenceRejected):
        verify(event, trust, artifacts, SCHEMA, slots / "active", bump_receipt, promotion_receipt)
