# CoordinatorProtocol/v1

`scripts/validate_coordinator_protocol.py` defines the neutral contract for
the coordinator boundary requested by issue #50 and revised by issue #201.
The external LLM that invokes Code is the sole cognitive authority for a
productive turn. Simplicio Code, Agent, Loop, and Runtime execute contracts,
isolation, queues, effects, observability, and validation; they do not create
or activate another cognitive coordinator. The schema retains `builtin` and
`simplicio-agent` coordinator values only for isolated diagnostics and contract
compatibility. Neither may enter the productive path. A productive turn names
`external` as its coordinator, `external-invoker` as its cognitive authority,
`simplicio-runtime` as its effect authority, and `none` as provider activation.
It carries stable workspace/session/turn/policy identities;
events must have unique increasing sequence numbers and causal IDs, and
invalid lifecycle transitions are blocked.

This contract is intentionally independent of the invoking LLM and Agent
implementations. The Agent remains in its own repository and does not import Code. Code's
`simplicio-agent-client` verifies the independent AgentHost's
`simplicio.agent-host/v1` discovery envelope, `agent/v1` protocol, mandatory
capabilities, and bounded `simplicio.agent-advisory/v1` replay before use. On
Windows, where `AF_UNIX` may be unavailable, it discovers the Agent's private
`127.0.0.1` endpoint and one-process bearer token from the Agent-owned
sidecars; non-loopback endpoints and malformed tokens are rejected.
`SimplicioRuntimeFs` verifies the operational AgentHost contract and Runtime,
failing closed if a required dependency is absent or incompatible. This
handshake grants operational capability only; it never transfers cognitive
authority or starts an internal provider/local LLM. Diagnostics that do not
execute a productive turn or project effect may still report dependency health
while a service is absent.

## Code adapter lifecycle

The Code surfaces share the operational `AgentHostCoordinator` adapter through
`xai-grok-agent`: TUI/headless, ACP, workspace, and the Runtime-backed
filesystem use one execution boundary rather than creating independent Agent
instances or a second LLM coordinator.
Each turn carries `CausalIdentity` (`workspace_id`, `session_id`, `turn_id`,
`attempt_id`, `idempotency_key`, `run_id`, `stage_id`, `fence`, and
`policy_revision`). The adapter allows one active turn, preserves its identity
for retry/cancel, exposes the replay cursor, and publishes
`simplicio.code-coordinator-snapshot/v1`.

If the host disappears or its incarnation changes before the effect is
reconciled, the adapter enters `effect_unknown`; it never silently retries or
executes a local effect. Reconnect returns a snapshot and replay resumes from
the stored cursor. Runtime filesystem edits and argv execution remain behind
`SimplicioRuntimeFs`, after the AgentHost handshake has succeeded.

Advisory replay is a passive attention surface, not a cognitive coordinator: it
contains only a fixed operational vocabulary, cannot carry prompts/workspace
content, cannot invoke Runtime effects, and must not steal terminal focus. The
Rust client exposes a minimal `AgentAttentionState` for a future side panel;
rendering and interactive approval/cancel/resume controls remain a subsequent
UI slice. Runtime remains the sole authority for filesystem and command
effects, and Simplicio Loop remains the ecosystem scheduler.

## Productive authority invariant

Every productive envelope is fail-closed unless all four statements are true:

1. `coordinator` is `external`;
2. `cognitive_authority` is `external-invoker`;
3. `effect_authority` is `simplicio-runtime`;
4. `provider_activation` is `none`.

The validator deliberately rejects `builtin` and `simplicio-agent` as
productive coordinators and rejects internal-provider, local-LLM, or fallback
activation. This issue does not implement the multi-session control plane,
agent DAGs/worktrees, routing, onboarding, or final parity E2E owned by issues
#202–#208. It freezes the parent architecture boundary those slices must obey.

The current advisory vocabulary is operational only. It does **not** observe
workspace activity and does not emit proactive `finding`/`risk`/`suggestion`
records. A neutral `workspace.observe` + `workspace.advisory` contract, its
privacy/approval policy, Agent-side production, Code-side polling/rendering,
and the complete multi-command AgentHost → `CoordinatorProtocol/v1` adapter
remain explicit follow-up work. The TUI now has one explicit productive entry
point, `/simplicio <instruction>`, which sends a causally identified
`turn.start` through the negotiated AgentHost off the event-loop thread and
renders its terminal result in scrollback. This is not a completed visual
panel, command inventory, or proactive-assistance surface.
