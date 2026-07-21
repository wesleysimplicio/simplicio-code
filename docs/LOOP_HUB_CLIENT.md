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

The contract prevents duplicate daemon declarations and provides the admission
gate needed by issue #55. The external Loop Hub daemon, cross-process endpoint
discovery, Mapper service, and transport adapter are not implemented in this
repository yet; a product adapter must supply `HubTransport` and connect to an
already-running Hub. Until then `required` mode fails closed.
