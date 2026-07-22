import io
import json

import pytest

from scripts.issue_meta_audit import SCHEMA, audit, fetch_all, main, render_markdown


def _issue(number=1, body="", **overrides):
    value = {
        "number": number,
        "title": f"Issue {number}",
        "state": "open",
        "created_at": f"2026-01-{number:02}T00:00:00Z",
        "closed_at": None,
        "labels": [],
        "body": body,
    }
    value.update(overrides)
    return value


COMPLETE_BODY = """## Contexto e problema
x
## Objetivo
x
## Fora de escopo
x
## Entradas, saídas e contratos
x
## Dependências e ordem
#2
## Passo a passo implementável
1. fazer
## Fluxo de testes
x
## Critérios de aceite verificáveis
85% cobertura medida
## Evidências obrigatórias
logs
## Riscos, rollback e decisão de encerramento
x
"""


def test_audit_orders_issues_excludes_prs_and_accepts_complete_template():
    result = audit(
        [_issue(2, COMPLETE_BODY), {**_issue(99, "Closes #1"), "pull_request": {"url": "pr"}}, _issue(1, COMPLETE_BODY)],
        "owner/repo",
        "fixture.json",
    )
    assert result["schema"] == SCHEMA
    assert [entry["number"] for entry in result["issues"]] == [1, 2]
    assert result["summary"]["reviewed_compliant"] == 2
    assert result["issues"][0]["references"] == [2]
    assert result["issues"][0]["linked_pull_requests"] == [99]
    assert result["issues"][0]["classification"]["priority"] == "unassigned"


def test_audit_fails_closed_with_precise_gaps_and_unmeasured_claim():
    result = audit([_issue(body="## Objetivo\nMelhorar performance\n")], "owner/repo", "fixture")
    entry = result["issues"][0]
    assert entry["decision"] == "needs-spec-rewrite"
    assert "context" in entry["gaps"]
    assert "measurement_evidence" in entry["gaps"]
    assert "numbered_steps" in entry["gaps"]


def test_markdown_records_counts_hash_and_reproduction_command():
    report = render_markdown(audit([_issue(body=COMPLETE_BODY)], "owner/repo", "fixture"))
    assert "**1** accessible issues" in report
    assert "Body SHA-256" in report
    assert "python3 scripts/issue_meta_audit.py" in report


def test_fetch_all_paginates_and_preserves_pull_request_entries():
    pages = [[_issue(number=i + 1) for i in range(100)], [{**_issue(101), "pull_request": {"url": "pr"}}]]
    calls = []

    class Response(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    def opener(request, timeout):
        calls.append((request.full_url, timeout, request.headers["User-agent"]))
        return Response(json.dumps(pages[len(calls) - 1]).encode())

    items = fetch_all("owner/repo", opener=opener)
    assert len(items) == 101
    assert "page=2" in calls[1][0]
    assert calls[0][1:] == (30, "simplicio-meta-audit/1")


def test_fetch_all_rejects_non_list_response():
    class Response(io.BytesIO):
        def __enter__(self):
            return self

        def __exit__(self, *args):
            return False

    with pytest.raises(ValueError, match="must be a list"):
        fetch_all("owner/repo", opener=lambda *args, **kwargs: Response(b"{}"))


def test_main_writes_both_reproducible_artifacts(tmp_path):
    source = tmp_path / "source.json"
    output = tmp_path / "nested" / "audit.json"
    report = tmp_path / "nested" / "audit.md"
    source.write_text(json.dumps([_issue(body=COMPLETE_BODY)]), encoding="utf-8")
    assert main(["--repository", "owner/repo", "--input", str(source), "--json", str(output), "--markdown", str(report)]) == 0
    assert json.loads(output.read_text())["summary"]["accessible_issues"] == 1
    assert "# Issue meta-audit report" in report.read_text()


def test_main_fails_closed_for_invalid_export(tmp_path):
    source = tmp_path / "source.json"
    source.write_text("{}", encoding="utf-8")
    with pytest.raises(SystemExit) as error:
        main(["--input", str(source), "--json", str(tmp_path / "out.json"), "--markdown", str(tmp_path / "out.md")])
    assert error.value.code == 2
