# Loop Hub client contract

`scripts/validate_loop_hub_client.py` emits
`simplicio.loop-hub-client/v1`. The typed Rust adapter in
`crates/codegen/simplicio-runtime-client/src/loop_hub.rs` uses the same schema.
It makes the ownership decision explicit for Code: with a ready Hub, Runtime,
Mapper, scheduler, and inference capacity have one Hub owner and Code reuses
the negotiated handles; `standalone` is an explicit mode, not a silent
fallback.

`LoopHubClient` also forwards interactive submissions with deterministic
session/turn/goal idempotency keys, queue admission/backpressure, progress
cursors, cancel, resume, and receipts. It never creates a scheduler, worker,
Runtime, Mapper, or inference process. `SharedRuntimeClient` prevents multiple
Code surfaces in one process from opening duplicate Runtime MCP children for
the same workspace.

`LoopHubClient::connect` uses the explicit endpoint in `HubClientConfig` first,
then the `SIMPLICIO_LOOP_HUB_ENDPOINT` environment variable. Product adapters
with a user/machine discovery mechanism can implement
`HubEndpointDiscovery` and call `connect_with_discovery`; the adapter is
required to return an endpoint for an already-running Hub and must not spawn a
daemon or scheduler. `HubTransport` remains the injected handshake and request
boundary, so endpoint discovery does not imply that Code owns the transport.

The contract prevents duplicate daemon declarations and provides the admission
gate needed by issue #55. The external Loop Hub daemon, cross-process endpoint
discovery contract, Mapper service, and transport adapter are not implemented
in this repository yet; a product adapter must supply both
`HubEndpointDiscovery`/`HubTransport` and connect to an already-running Hub.
Until then `required` mode fails closed. The Rust tests cover this seam with a
fake already-running Hub and assert the exact v1 handshake request; they do not
start a daemon, scheduler, or local resource queue.
