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
