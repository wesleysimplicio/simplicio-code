# AgentHost + Runtime preflight

Code has one productive dependency gate: an independently installed
Simplicio AgentHost must negotiate the `simplicio.agent-host/v1` contract before
the Runtime-backed tools are used, and the Simplicio Runtime must negotiate
`simplicio.code-mcp/v1` with the complete Code tool vocabulary.

The Code-side diagnostic is exposed by
`xai_grok_tools::computer::local::SimplicioRuntimeFs::preflight`. It returns
`simplicio.code-agent-runtime-preflight/v1` with stable component statuses and
codes:

- `agent_host.missing` means the installed AgentHost socket was not found.
- `agent_host.incompatible` means transport, profile, protocol, readiness, or
  capability validation failed.
- `runtime.missing` means no installed Runtime executable was found.
- `runtime.incompatible` means the Runtime could not complete its handshake.
- `runtime.capabilities_missing` lists the sorted required Code tools.
- `causal_identity.invalid` means the turn and idempotency identity is not
  complete or internally consistent.

Diagnostics do not include socket paths, process IDs, or arbitrary process
output. This keeps support output deterministic across machines.

## Offline fixture boundary

`OfflineContractFixture::validate` is available for unit and protocol tests.
It validates schema/version serialization and causal identity only. Its
component status is `protocol_only`, `effects_enabled` is always `false`, and
`can_enter_productive_mode()` is always false. It must never be used to claim
that an installed AgentHost + Runtime E2E succeeded.

The fixture does not start a process, connect to a socket, invoke an MCP tool,
write or delete files, execute a command, or provide a built-in/local effect
fallback.

## Productive boundary

Preflight is diagnostic and does not grant authority. Productive operations
remain behind `SimplicioRuntimeFs::with_runtime`: AgentHost is checked first,
Runtime owns the filesystem/exec effect, and any missing or incompatible
dependency fails closed. A valid offline fixture cannot satisfy that gate.

An actual installed E2E receipt still requires an installed AgentHost and
Runtime fixture on the target operating system; this change intentionally does
not manufacture that evidence.
