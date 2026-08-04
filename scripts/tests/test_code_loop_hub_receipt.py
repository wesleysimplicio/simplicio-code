import json
from pathlib import Path


ROOT = Path(__file__).parents[2]
RECEIPT = ROOT / "dist" / "code-loop-hub-e2e-issue-319.json"


def test_committed_loop_hub_receipt_is_external_and_provider_free():
    payload = json.loads(RECEIPT.read_text(encoding="utf-8"))
    assert payload["schema"] == "simplicio.code-loop-hub-e2e/v1"
    assert payload["proof_kind"] == "external_loop_daemon"
    assert payload["single_hub_identity"] is True
    assert payload["restart_reconnected"] is True
    assert payload["hub_pid_rotated"] is True
    assert payload["provider_free"] is True
    assert payload["local_llm_started"] is False
    assert payload["runtime_started_by_code"] is False
    assert payload["mapper_started_by_code"] is False
    assert payload["scheduler_started_by_code"] is False
    assert payload["surfaces"] == ["tui-1", "tui-2", "headless", "acp"]
