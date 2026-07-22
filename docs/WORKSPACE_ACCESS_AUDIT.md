# Workspace access audit

`docs/contracts/workspace-access-manifest.json` is the baseline for the
Runtime-boundary audit from issue #49. `scripts/audit_workspace_access.py`
scans the Simplicio-owned source scopes and emits
`simplicio.workspace-access-manifest/v1`.

Every direct filesystem, process, or tree-walk call site must have an owner,
rationale, and classification. `violation` and unclassified findings fail the
gate; test fixtures and the short bootstrap allowlist remain explicit. The
manifest intentionally records the current `xai-grok-workspace` bypasses as
violations so the audit cannot be mistaken for proof that the migration is
complete. This includes auxiliary attachment, indexing, tree, fuzzy-search and
walk paths even though the productive agent tools (`grep`, `hashline_grep`,
`grep_files`, `list_dir`, apply/edit and terminal execution) are Runtime-wired.
Consequently, a failing audit is expected evidence of the remaining in-repo
work, not an external dependency failure and not a releasable acceptance
receipt.

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
