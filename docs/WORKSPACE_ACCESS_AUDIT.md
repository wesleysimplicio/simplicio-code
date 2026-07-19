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
complete.

```bash
python3 scripts/audit_workspace_access.py --json
```
