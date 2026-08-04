# Workspace access audit

`docs/contracts/workspace-access-manifest.json` is the baseline for the
Runtime-boundary audit from issue #49. `scripts/audit_workspace_access.py`
scans the Simplicio-owned source scopes and emits
`simplicio.workspace-access-manifest/v1`.

Every direct filesystem, process, or tree-walk call site must have an owner,
rationale, and classification. The audit rejects manifest rules that omit a
non-empty owner or rationale before scanning source. `violation` and unclassified findings fail the
gate; test fixtures and the short bootstrap allowlist remain explicit. Historical violation entries are retained only as zero-count inventory
keys; a positive `max_count` for classification `violation` fails closed.
The current scoped surfaces route workspace list/stat/read/write/delete,
recursive listing, indexing and fuzzy enumeration through Runtime-backed
adapters; direct filesystem calls in the scanned source are test fixtures or
bounded session scratch. This scoped audit is not a cross-platform installed
E2E receipt, so broader release and package evidence remains separate.

The optional `baseline` is an upper bound keyed by path, access kind, and
classification. It prevents a broad reviewed rule from silently accepting a
new call site while allowing bypass removal to reduce the count. Missing keys
and counts above `max_count` fail closed as `baseline_errors`.

```bash
python3 scripts/audit_workspace_access.py
```

For a compact, reproducible blocker summary without discarding the complete
JSON receipt:

```bash
python3 scripts/audit_workspace_access.py > workspace-access-audit.json
python3 -c 'import json; d=json.load(open("workspace-access-audit.json")); print(d["status"], d["summary"])'
```
