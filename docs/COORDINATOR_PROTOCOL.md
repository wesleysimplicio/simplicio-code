# CoordinatorProtocol/v1

`scripts/validate_coordinator_protocol.py` defines the neutral contract for
the coordinator boundary requested by issue #50. Simplicio Agent is an
independent product and the mandatory cognitive coordinator for productive
Simplicio Code turns. The schema retains `builtin` and `external` only for
isolated diagnostics/contract compatibility; neither may enter the productive
effect path. A productive turn therefore names exactly one coordinator:
`simplicio-agent`. It carries stable workspace/session/turn/policy identities;
events must have unique increasing sequence numbers and causal IDs, and
invalid lifecycle transitions are blocked.

This contract is intentionally independent of the Agent implementation. The
Agent remains in its own repository and does not import Code. Code's
`simplicio-agent-client` verifies the independent AgentHost's
`simplicio.agent-host/v1` discovery envelope, `agent/v1` protocol, mandatory
capabilities, and bounded `simplicio.agent-advisory/v1` replay before use. On
Windows, where `AF_UNIX` may be unavailable, it discovers the Agent's private
`127.0.0.1` endpoint and one-process bearer token from the Agent-owned
sidecars; non-loopback endpoints and malformed tokens are rejected.
`SimplicioRuntimeFs` verifies Agent first and Runtime second, failing closed if
either dependency is absent or incompatible. Diagnostics that do not execute a
productive turn or project effect may still report dependency health while a
service is absent.

## Code adapter lifecycle

The Code surfaces share `AgentHostCoordinator` through the `xai-grok-agent`
adapter: TUI/headless, ACP, workspace, and the Runtime-backed filesystem use
one coordinator boundary rather than creating independent Agent instances.
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

Advisory replay is a passive attention surface, not a second coordinator: it
contains only a fixed operational vocabulary, cannot carry prompts/workspace
content, cannot invoke Runtime effects, and must not steal terminal focus. The
Rust client exposes a minimal `AgentAttentionState` for a future side panel;
rendering and interactive approval/cancel/resume controls remain a subsequent
UI slice. Runtime remains the sole authority for filesystem and command
effects, and Simplicio Loop remains the ecosystem scheduler.

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
