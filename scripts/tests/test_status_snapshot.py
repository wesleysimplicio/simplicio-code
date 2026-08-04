from __future__ import annotations

import importlib.util
from pathlib import Path


ROOT = Path(__file__).parents[2]
SPEC = importlib.util.spec_from_file_location("status_snapshot", ROOT / "scripts" / "status_snapshot.py")
assert SPEC and SPEC.loader
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_normalize_beta_versions() -> None:
    assert MODULE.normalize("0.3.0-beta.3") == "0.3.0b3"
    assert MODULE.normalize("0.3.0b3") == "0.3.0b3"


def test_current_sources_are_consistent() -> None:
    data = MODULE.snapshot(ROOT)
    assert data["source_version_status"] == "PASS"
    assert data["normalized_versions"] == {
        "python": "0.3.0b3",
        "rust": "0.3.0b3",
        "readme": "0.3.0b3",
        "onboarding_bundle": "0.3.0b3",
    }

def test_bundle_source_and_migration_note_are_checked() -> None:
    data = MODULE.snapshot(ROOT)
    assert data["versions"]["onboarding_bundle"] == "0.3.0-beta.3"
    MODULE.validate_rendered_document(ROOT, data)


def test_dirty_checkout_is_not_release_evidence() -> None:
    data = MODULE.snapshot(ROOT)
    assert data["release_evidence_status"] == "UNKNOWN"


def test_rendered_status_document_matches_sources() -> None:
    data = MODULE.snapshot(ROOT)
    MODULE.validate_rendered_document(ROOT, data)
