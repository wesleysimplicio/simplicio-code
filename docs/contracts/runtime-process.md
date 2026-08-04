# Runtime process lifecycle contract

Code is a client of the Loop Hub; it does not spawn, supervise, retry, or
reconcile a process locally.

The Hub handshake may advertise the following independent capabilities under
`runtime_process`:

- `start`: `process.start/v1`
- `status`: `process.status/v1`
- `cancel`: `process.cancel/v1`
- `wait`: `process.wait/v1`

An omitted capability is false. Code rejects the request with an actionable
`TransportUnavailable` error before sending an effect. The client validates
the schema, workspace, handle, argv, idempotency key, and bounded deadline
before capability dispatch.

The lifecycle state is one of `not_started`, `running`, `exited`,
`cancelled`, `timed_out`, or `effect_unknown`. `effect_unknown` is never
automatically retried. The caller must query the Runtime by the returned
handle and causal receipt before attempting another start.

The external transport methods are `process_start`, `process_status`,
`process_cancel`, and `process_wait`. A transport that does not implement a
method returns `TransportUnavailable`; there is no local `Command`, shell, or
fallback process path in this adapter.

This Code-side contract is not evidence that a production Runtime implements
the capabilities. Installed Runtime/Hub evidence, adversarial E2E receipts,
and platform coverage remain required before issue #317 can be closed.
