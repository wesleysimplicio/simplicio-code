import importlib.util
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location("policy_scan", ROOT / "tools" / "policy_scan.py")
MODULE = importlib.util.module_from_spec(SPEC)
assert SPEC.loader is not None
SPEC.loader.exec_module(MODULE)


def policy(removal_date="2099-01-01"):
    return f'''\
schema = "simplicio.no-internal-json/v1"
version = 1
scanner_version = "0.1.0"

[[exceptions]]
path = "external.py"
category = "external-boundary"
owner = "owner"
external_dependency = "dependency"
justification = "required wire protocol"
review_date = "2026-01-01"
removal_date = "{removal_date}"
'''


def boundaries(*entries):
    rendered = [
        'schema = "simplicio.json-boundaries/v1"',
        "",
    ]
    for path, status, expires in entries:
        rendered.extend(
            [
                "[[boundary]]",
                f'path = "{path}"',
                'category = "boundary-category"',
                'owner = "owner"',
                'producer = "producer"',
                'consumer = "consumer"',
                'lifecycle = "lifecycle"',
                'target_format = "HBI"',
                f'status = "{status}"',
                'reason = "reason"',
                f'expires = "{expires}"',
                "",
            ]
        )
    return "\n".join(rendered)


def test_strict_scan_fails_unclassified_json_and_accepts_exact_exception(tmp_path):
    (tmp_path / "internal.py").write_text("json.dumps(value)\n", encoding="utf-8")
    (tmp_path / "external.py").write_text("json.dumps(value)\n", encoding="utf-8")
    policy_path = tmp_path / "policy.toml"
    policy_path.write_text(policy(), encoding="utf-8")

    loaded = MODULE.load_policy(policy_path, "2026-07-22")
    findings = MODULE.scan(tmp_path, loaded)
    markdown, hbp, code = MODULE.render(findings, loaded, "strict")

    assert code == 1
    assert "status: `FAIL`" in markdown
    assert "internal.py" in markdown
    assert "schema=simplicio.hbp/v1" in hbp


def test_expired_exception_is_rejected(tmp_path):
    policy_path = tmp_path / "policy.toml"
    policy_path.write_text(policy("2026-07-21"), encoding="utf-8")
    with pytest.raises(ValueError, match="expired exception"):
        MODULE.load_policy(policy_path, "2026-07-22")


def test_inventory_classification_keeps_pending_and_unknown_non_passing(tmp_path):
    (tmp_path / "pending.py").write_text("json.dumps(value)\n", encoding="utf-8")
    (tmp_path / "known.py").write_text("json.dumps(value)\n", encoding="utf-8")
    (tmp_path / "unknown.py").write_text("json.dumps(value)\n", encoding="utf-8")
    policy_path = tmp_path / "policy.toml"
    policy_path.write_text(policy(), encoding="utf-8")
    boundaries_path = tmp_path / "boundaries.toml"
    boundaries_path.write_text(
        boundaries(
            ("pending.py", "migration_pending", "2099-01-01"),
            ("known.py", "exception", "2099-01-01"),
        ),
        encoding="utf-8",
    )

    loaded = MODULE.load_policy(policy_path, "2026-07-22")
    inventory = MODULE.load_boundaries(boundaries_path)
    findings = MODULE.scan(tmp_path, loaded, inventory)
    baseline, _, baseline_code = MODULE.render(findings, loaded, "baseline", inventory)
    strict, hbp, strict_code = MODULE.render(findings, loaded, "strict", inventory)

    assert baseline_code == 0
    assert "status: `UNVERIFIED`" in baseline
    assert "unknown: `1`" in baseline
    assert "pending: `1`" in baseline
    assert "known.py" in baseline
    assert "Classification | State" in baseline
    assert strict_code == 1
    assert "status: `FAIL`" in strict
    assert "schema=simplicio.hbp/v1" in hbp


def test_inventory_expiry_is_reported_separately_and_fails_strict(tmp_path):
    (tmp_path / "expired.py").write_text("json.dumps(value)\n", encoding="utf-8")
    policy_path = tmp_path / "policy.toml"
    policy_path.write_text(policy(), encoding="utf-8")
    boundaries_path = tmp_path / "boundaries.toml"
    boundaries_path.write_text(
        boundaries(("expired.py", "exception", "2026-07-21")),
        encoding="utf-8",
    )

    loaded = MODULE.load_policy(policy_path, "2026-07-22")
    inventory = MODULE.load_boundaries(boundaries_path)
    findings = MODULE.scan(tmp_path, loaded, inventory)
    markdown, hbp, code = MODULE.render(findings, loaded, "strict", inventory)

    assert code == 1
    assert "unknown: `0`" in markdown
    assert "pending: `0`" in markdown
    assert "expired: `1`" in markdown
    assert "| `expired.py` | 1 | `serialization-call` | `boundary-category` | `expired` |" in markdown
    assert "expired=1" in hbp
