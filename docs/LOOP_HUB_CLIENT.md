# Loop Hub client contract

`scripts/validate_loop_hub_client.py` emits
`simplicio.loop-hub-client/v1`. The typed Rust adapter in
`crates/codegen/simplicio-runtime-client/src/loop_hub.rs` uses the same schema.
It makes the ownership decision explicit for Code: with a ready Hub, Runtime,
Mapper, scheduler, and inference capacity have one Hub owner and Code reuses
the negotiated handles; `standalone` is an explicit mode, not a silent
fallback. `standalone` means only that a simple operation does not create a
Loop workflow; it never removes the production requirement for a compatible
Simplicio Agent and Runtime.

`LoopHubClient` also forwards interactive submissions with deterministic
session/turn/goal idempotency keys, queue admission/backpressure, progress
cursors, cancel, resume, and receipts. It never creates a scheduler, worker,
Runtime, Mapper, or inference process. `SharedRuntimeClient` prevents multiple
Code surfaces in one process from opening duplicate Runtime MCP children for
the same workspace.

UI and headless adapters can retain `LoopHubClient::interactive_transport()`
as an interactive-only capability: it can submit validated interactive goals
but cannot access the raw transport or select another priority. The
`shared_runtime_handle()` and `shared_map_handle()` accessors expose the
Hub-negotiated service identity and capacity. These cloneable handles retain
the same negotiated session and provide `shares_session_with()` so an adapter
can verify that Map and Runtime came from one Hub rather than comparing
fallible string identifiers. None of these accessors opens a connection or
starts a local service.

Connections are reused only by clones with the same endpoint, workspace, and
logical session. A second TUI, headless process, or ACP session attaches its
own logical session to the same external Hub endpoint. This is intentional:
the wire-level `attach` receipt admits one `session_id`, so reusing that
transport for a different session would submit work under an identity the Hub
never admitted. The surfaces still share the Hub-owned Runtime, Mapper,
scheduler, and inference identities reported by their handshakes; Code does
not create any of those services.

`LoopHubClient::connect` uses the explicit endpoint in `HubClientConfig` first,
then the `SIMPLICIO_LOOP_HUB_ENDPOINT` environment variable. Product adapters
with a user/machine discovery mechanism can implement
`HubEndpointDiscovery` and call `connect_with_discovery`; the adapter is
required to return an endpoint for an already-running Hub and must not spawn a
daemon or scheduler. `HubTransport` remains the injected handshake and request
boundary, so endpoint discovery does not imply that Code owns the transport.

## External socket/pipe transport

`SocketPipeHubTransportFactory` is the standard Code-side transport adapter.
It accepts `unix:///absolute/path/to/loop-hub.sock` on Unix and
`pipe://name` on Windows. It opens an already-running endpoint; it never
spawns a daemon, scheduler, mapper, Runtime, model worker, or local queue.
Unknown endpoint schemes and unsupported platforms fail closed.

The wire contract is newline-delimited JSON at the external boundary. Every
frame has `schema`, a monotonic connection-local `id`, `method`, and `payload`;
responses must echo the schema and id and contain either `ok/result` or an
error. The required sequence is:

1. `handshake` with `simplicio.loop-hub-client/v1` and
   `simplicio.loop-hub/v1`.
2. `attach` with the same client/workspace/session identity and
   `reconnect=false`.
3. `submit`, `progress`, `cancel`, and `resume` calls through the Hub-owned
   queue and services.

If the connection drops, Code opens the same endpoint again, repeats the
versioned handshake, then sends `attach` with `reconnect=true` and the last
`after_sequence` cursor for every workflow. Progress reads are safe to replay
with that cursor. Submit, cancel, and resume are deliberately not retried
after a broken connection: their receipt may be unknown, so the client
reattaches and returns a fail-closed error instead of duplicating an effect.

The adapter validates a ready, versioned external Agent and that the Hub owns
Runtime, Mapper, scheduler, and inference, exposes bounded
interactive/background capacity, and declares one active inference slot. A
handshake or attach mismatch blocks the client.

This completes the Code-side transport boundary for #55/#106. The external
Loop Hub daemon/endpoint provider, Mapper service, queue/fairness enforcement,
and real multi-surface evidence remain outside this repository. The acceptance
issues must stay open until two TUIs plus headless and ACP attach to the same
running Hub and receipts/process counts prove the no-duplication invariant.
The contract tests use an in-process Unix socket fixture only; they do not
pretend to be external Loop Hub evidence.

Coordinated goals (waves, multiple issues, global queues, or parallel work)
must use a ready Hub. The admission validator rejects `coordinated` scope when
the Hub is absent rather than falling back to Code-local fan-out. The natural
request “finish all issues for project X” is therefore routed Code → Agent →
Loop Hub → Runtime/Mapper/workers; completion still requires a remotely
requeried delivery receipt from the external ecosystem.

## External worker adapter

`agent_workers` adds the Code-side contract for materialized agents without
turning Code into a second scheduler. Code submits a bounded task DAG as one
`DelegateRequest`; the external Loop Hub owns wave admission, claims, leases,
backpressure, retries, and worker creation. Task contracts remain opaque input
for the external AgentHost. The adapter has no model/provider selection,
process execution, worktree creation, or local scheduling API.

Hub events carry stage/agent/worktree/attempt/fence identity. Code reduces the
causal stream into `waiting`, `working`, `blocked`, `failed`, `done`, or
`cancelled` UI state while rejecting stale fences, invalid transitions,
duplicate events, and simultaneous worktree/branch/path-token ownership. A
`done` state needs a receipt; a live worker alone is never success. STOP/cancel
always asks the Hub to revoke mutation authority.

Delivery is a separate, replay-safe request. An implementer cannot issue it:
the observed worker must have the `delivery` role, be terminal with a valid
receipt, and present an independent review receipt. Code accepts completion
only when the Hub's response includes a non-empty remote reference and
`remotely_confirmed=true`. The adapter does not merge or close an issue.

`WorkerHubTransport` is the external seam. Production binaries must bind it to
the installed Loop Hub transport; unit tests use a fake only to exercise the
Code-side validator and reducer. Absence of that binding fails closed and must
never activate a local worker, provider, or embedded LLM.
