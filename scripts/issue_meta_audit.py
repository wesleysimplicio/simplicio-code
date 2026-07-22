#!/usr/bin/env python3
"""Build a reproducible, fail-closed audit of GitHub issue specifications.

The audit intentionally consumes an exported GitHub API response as well as
live API pages.  Keeping the analysis pure makes the result reviewable without
credentials or paid GitHub Actions and avoids sending issue text anywhere.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import urllib.error
import urllib.request
from collections import Counter
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Callable

SCHEMA = "simplicio.issue-meta-audit/v1"
REQUIRED_SECTIONS: dict[str, tuple[str, ...]] = {
    "context": ("context", "contexto", "problema", "problem", "summary", "resumo"),
    "objective": ("objective", "objetivo", "resultado esperado", "expected"),
    "out_of_scope": ("fora de escopo", "não objetivo", "nao objetivo", "non-goal"),
    "contracts": ("entrada", "saída", "saida", "contract", "contrato"),
    "dependencies": ("depend", "ordem", "related", "refer"),
    "steps": ("passo a passo", "implementation step", "plano", "investigation step", "reprodução", "reproduction"),
    "test_flow": ("test", "teste", "verification", "verificação", "verificacao"),
    "acceptance": ("acceptance", "critério", "criterio"),
    "evidence": ("evidence", "evidência", "evidencia", "receipt", "log"),
    "risk_rollback_decision": ("risk", "risco", "rollback", "encerramento", "closure", "decision", "decisão", "decisao"),
}
MEASUREMENT_CLAIMS = re.compile(r"\b(cobertura|coverage|lat[eê]ncia|latency|desempenho|performance|economia|saving|integra[cç][aã]o|integration)\b", re.I)
MEASUREMENT_EVIDENCE = re.compile(r"(?:\b\d+(?:[.,]\d+)?\s*(?:%|ms\b|s\b|Mi?B\b|Gi?B\b|tokens?\b)|benchmark|m[eé]trica|metric|p50|p95)", re.I)
NUMBERED_STEP = re.compile(r"(?m)^\s*\d+[.)]\s+\S")
ISSUE_REFERENCE = re.compile(r"(?<![\w/])#(\d+)\b")


def _classification(item: dict[str, Any], body: str) -> dict[str, str]:
    text = f"{item.get('title', '')}\n{body}".casefold()
    priority_match = re.search(r"\b(p[0-3])\b", text)
    component_rules = (
        ("runtime-boundary", ("runtime", "mcp", "workspace")),
        ("agent-orchestration", ("agent", "loop", "coordenador", "orchestration")),
        ("release-ci", ("release", "ci", "build", "coverage")),
        ("security-privacy", ("security", "segredo", "secret", "privacy", "telemetr")),
        ("product-ui", ("tui", "painel", "tema", "brand", "login")),
    )
    component = next((name for name, words in component_rules if any(word in text for word in words)), "repository-governance")
    risk = "critical" if (priority_match and priority_match.group(1).upper() == "P0") or "security" in text else "standard"
    epic = "product-1.0" if any(token in text for token in ("1.0", "epic", "product e2e")) else "continuous-hardening"
    return {"epic": epic, "component": component, "risk": risk, "priority": priority_match.group(1).upper() if priority_match else "unassigned"}


def fetch_all(repo: str, opener: Callable[..., Any] = urllib.request.urlopen) -> list[dict[str, Any]]:
    """Fetch every issue API page, retaining PR entries for relationship data."""
    items: list[dict[str, Any]] = []
    page = 1
    while True:
        url = f"https://api.github.com/repos/{repo}/issues?state=all&direction=asc&per_page=100&page={page}"
        request = urllib.request.Request(url, headers={"Accept": "application/vnd.github+json", "User-Agent": "simplicio-meta-audit/1"})
        with opener(request, timeout=30) as response:
            batch = json.load(response)
        if not isinstance(batch, list):
            raise ValueError("GitHub issues response must be a list")
        items.extend(batch)
        if len(batch) < 100:
            return items
        page += 1


def _headings(body: str) -> list[str]:
    return [match.strip() for match in re.findall(r"(?m)^#{1,6}\s+(.+?)\s*$", body)]


def _has_section(headings: list[str], aliases: tuple[str, ...]) -> bool:
    normalized = "\n".join(headings).casefold()
    return any(alias.casefold() in normalized for alias in aliases)


def audit(items: list[dict[str, Any]], repository: str, source: str) -> dict[str, Any]:
    issues = [item for item in items if "pull_request" not in item]
    issues.sort(key=lambda item: (str(item.get("created_at", "")), int(item["number"])))
    issue_numbers = {int(item["number"]) for item in issues}
    pull_requests = [item for item in items if "pull_request" in item]
    records: list[dict[str, Any]] = []
    for item in issues:
        body = str(item.get("body") or "")
        headings = _headings(body)
        sections = {name: _has_section(headings, aliases) for name, aliases in REQUIRED_SECTIONS.items()}
        references = sorted({int(value) for value in ISSUE_REFERENCE.findall(body) if int(value) in issue_numbers})
        linked_prs = sorted(
            int(pull["number"])
            for pull in pull_requests
            if int(item["number"]) in {int(value) for value in ISSUE_REFERENCE.findall(str(pull.get("body") or ""))}
        )
        labels = sorted(str(label.get("name", "")) for label in item.get("labels", []) if isinstance(label, dict))
        missing = [name for name, present in sections.items() if not present]
        measured = not MEASUREMENT_CLAIMS.search(body) or bool(MEASUREMENT_EVIDENCE.search(body))
        numbered = bool(NUMBERED_STEP.search(body))
        compliant = not missing and numbered and measured
        records.append(
            {
                "number": int(item["number"]),
                "title": str(item.get("title", "")),
                "state": str(item.get("state", "unknown")),
                "created_at": str(item.get("created_at", "")),
                "closed_at": item.get("closed_at"),
                "labels": labels,
                "references": references,
                "linked_pull_requests": linked_prs,
                "classification": _classification(item, body),
                "sections": sections,
                "numbered_steps": numbered,
                "measurement_evidence": measured,
                "body_sha256": hashlib.sha256(body.encode()).hexdigest(),
                "decision": "reviewed-compliant" if compliant else "needs-spec-rewrite",
                "gaps": missing + ([] if numbered else ["numbered_steps"]) + ([] if measured else ["measurement_evidence"]),
            }
        )

    states = Counter(record["state"] for record in records)
    compliant_count = sum(record["decision"] == "reviewed-compliant" for record in records)
    return {
        "schema": SCHEMA,
        "repository": repository,
        "source": source,
        "generated_at": max(
            (str(item.get("updated_at")) for item in items if item.get("updated_at")),
            default=datetime.now(timezone.utc).replace(microsecond=0).isoformat(),
        ),
        "summary": {
            "accessible_issues": len(records),
            "states": dict(sorted(states.items())),
            "reviewed_compliant": compliant_count,
            "needs_spec_rewrite": len(records) - compliant_count,
            "inventory_complete": True,
        },
        "issues": records,
    }


def render_markdown(result: dict[str, Any]) -> str:
    summary = result["summary"]
    lines = [
        "# Issue meta-audit report",
        "",
        f"- Repository: `{result['repository']}`",
        f"- Source: `{result['source']}`",
        f"- Generated: `{result['generated_at']}`",
        f"- Schema: `{result['schema']}`",
        "",
        "## Decision",
        "",
        f"Inventoried **{summary['accessible_issues']}** accessible issues in creation order "
        f"({summary['states'].get('open', 0)} open, {summary['states'].get('closed', 0)} closed). "
        f"**{summary['reviewed_compliant']}** pass the machine-verifiable specification gate and "
        f"**{summary['needs_spec_rewrite']}** require a body rewrite. A closed issue that fails this "
        "gate is not treated as complete. GitHub body edits and cross-project links require a "
        "credentialed owner action; this report never silently marks them complete.",
        "",
        "## Gate",
        "",
        "Each body must have headings covering context/problem, objective, out of scope, "
        "inputs/outputs/contracts, dependencies/order, implementation steps, test flow, measurable "
        "acceptance, evidence, and risks/rollback/closure. It must contain numbered steps; claims "
        "about coverage, performance, savings, or integration must include a measurement marker.",
        "",
        "## Inventory and closure decision",
        "",
        "| Created | Issue | State | Group (epic/component/risk/priority) | Labels | References / PRs | Decision | Gaps | Body SHA-256 |",
        "|---|---:|---|---|---|---|---|---|---|",
    ]
    for issue in result["issues"]:
        labels = ", ".join(issue["labels"]) or "—"
        refs = ", ".join(f"#{number}" for number in issue["references"]) or "—"
        prs = ", ".join(f"PR #{number}" for number in issue["linked_pull_requests"]) or "—"
        group = "/".join(issue["classification"].values())
        gaps = ", ".join(issue["gaps"]) or "—"
        lines.append(
            f"| {issue['created_at']} | [#{issue['number']}](https://github.com/{result['repository']}/issues/{issue['number']}) "
            f"| {issue['state']} | {group} | {labels} | {refs}; {prs} | `{issue['decision']}` | {gaps} | `{issue['body_sha256']}` |"
        )
    lines.extend(
        [
            "",
            "## Reproduction and required evidence",
            "",
            "Regenerate from the checked-in API export with:",
            "",
            "```sh",
            "python3 scripts/issue_meta_audit.py --input docs/audits/issue-139-source.json --json docs/audits/issue-139-inventory.json --markdown docs/audits/issue-139-report.md",
            "```",
            "",
            "A closure decision additionally requires links to the implementation PR/commit, test logs, "
            "failure-injection logs, receipts/hashes and measured metrics in each affected issue. Missing "
            "evidence keeps the issue in `needs-spec-rewrite`; rollback is to revert body changes using "
            "the recorded SHA-256/export and reopen any issue closed without evidence.",
            "",
        ]
    )
    return "\n".join(lines)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--repository", default="wesleysimplicio/simplicio-code")
    parser.add_argument("--input", type=Path, help="GitHub /issues response exported as a JSON list")
    parser.add_argument("--json", type=Path, required=True)
    parser.add_argument("--markdown", type=Path, required=True)
    args = parser.parse_args(argv)
    try:
        if args.input:
            items = json.loads(args.input.read_text(encoding="utf-8"))
            source = args.input.as_posix()
        else:
            items = fetch_all(args.repository)
            source = f"https://api.github.com/repos/{args.repository}/issues?state=all"
        if not isinstance(items, list):
            raise ValueError("input must be a JSON list")
        result = audit(items, args.repository, source)
        args.json.parent.mkdir(parents=True, exist_ok=True)
        args.markdown.parent.mkdir(parents=True, exist_ok=True)
        args.json.write_text(json.dumps(result, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
        args.markdown.write_text(render_markdown(result), encoding="utf-8")
    except (OSError, ValueError, json.JSONDecodeError, urllib.error.URLError) as exc:
        parser.exit(2, f"issue meta-audit failed: {exc}\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
