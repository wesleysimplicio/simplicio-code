# Workspace observation contract

The Code-side boundary is `simplicio.workspace.observe/v1`. Observation is
opt-in per workspace session through `WorkspaceObserveConsent`:

- `off` returns an empty page locally and does not contact AgentHost;
- `read_only` permits bounded observations only when AgentHost advertises
  `workspace.observe`.

An observation contains only a cursor, workspace identity, fixed category,
producer, opaque evidence handle, generation, and timestamp. The allowed
categories are `test_failure`, `stale_contract`, and `acceptance_gap`; allowed
producers are `mapper`, `runtime`, and `agent`. Arbitrary text and source
content are rejected by the Code client.

The client enforces contiguous cursors (unless the host marks the first item
as a truncated replay), workspace/host-instance provenance, generation
presence, and a maximum of 128 observations per page. Advisories remain
presentation-only. An observation cannot execute, schedule, approve, or inject
a tool call; any action requires an explicit Agent turn, policy approval, and
the Runtime effect boundary.

This contract is a Code-side gate, not proof that a production AgentHost emits
observations. AgentHost capability negotiation and four-surface E2E evidence
are still required before closing issue #326.
