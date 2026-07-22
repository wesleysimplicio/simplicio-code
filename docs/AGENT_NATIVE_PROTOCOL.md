# Agent-native protocol

`simplicio-agent-native` is the transport-neutral, versioned boundary used by
CLI, MCP, ACP, workspace, and optional AXI adapters. Every adapter submits the
same request operation and receives the same stable reason codes; an AXI tool
is an adapter, never a runtime dependency.

The invoking external LLM remains the cognitive authority. Code does not
interpret or expand its goal and this protocol cannot enable an internal
provider or local model. Agent validates the turn, Loop Hub owns scheduling,
and Runtime owns workspace effects. Missing dependencies therefore fail closed
instead of selecting a local fallback.

Discovery returns `simplicio.agent-native-capabilities/v1`. `doctor --json`
returns `installed`, `compatible`, `ready`, `degraded`, or `missing`, plus only
safe diagnostic commands. List and event methods use opaque cursors and reject
zero/oversized pages. The N-1 compatibility rule is expressed by the manifest's
`protocol_versions`; an unlisted schema is rejected.

The source-tree diagnostic can be exercised with `cargo run -p
simplicio-agent-native -- doctor --json`; `capabilities` prints the manifest.
The diagnostic only discovers configured/PATH executables and does not start
Agent, Loop Hub, Runtime, a provider, or a model.

External browser, pull-request, and delivery operations are governed intents.
They require an approval receipt, policy revision, and expected remote revision
for re-query before an adapter may ask the existing Agent/Loop/Runtime stack to
perform the effect. `effect_unknown` is terminal and must not be retried.

Receipts contain IDs, effect state, and redacted metadata only. Adapters must
apply the shared recursive redactor before logging or persisting untrusted
payloads; tokens, authorization, prompts, source code, and content are removed.

## External invoker flow

1. Read the capability manifest and run `doctor --json`.
2. Submit a caller-authored goal with `authority.kind=external_llm`.
3. Attach using the returned session ID and follow events with an opaque cursor.
4. Resolve `approval_required` through the existing approval surface.
5. Query the receipt by ID. Never infer completion from a disconnected stream.
6. For a remote effect, re-query remote state and supply its revision in the
   approved intent. Do not retry `effect_unknown`.

No step copies coordinator state into the client or grants the adapter its own
scheduler, executor, or model.
