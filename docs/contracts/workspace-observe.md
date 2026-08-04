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
presence, and a maximum of 128 observations per page. `WorkspaceObservationState`
adds the consumer-side cursor/generation guard and retains at most 128
observations before projection. The neutral simplicio.workspace.advisory/v1
state is bounded to 128 entries, accepts only finding, risk, and suggestion
from mapper, runtime, or agent, and requires workspace provenance, generation,
opaque evidence, and bounded confidence before projection. Consent off returns
without applying a page. An observation or advisory cannot execute, schedule,
approve, or inject a tool call; any action requires an explicit Agent turn,
policy approval, and the Runtime effect boundary.

The machine-readable request and page contracts are
`docs/contracts/workspace-observe-request-v1.schema.json` and
`docs/contracts/workspace-observe-v1.schema.json`. They are Code-side gates,
not proof that a production AgentHost emits observations. AgentHost capability
negotiation, the neutral advisory surface, and four-surface E2E evidence are
still required before closing issue #326.
