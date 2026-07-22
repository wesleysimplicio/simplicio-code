# External LLM budget and routing contract

`external_routing` is Code's admission/recommendation boundary for issue #205. It does **not**
invoke a model, initialize a provider, contain credentials, or permit an internal/local fallback.
The invoking LLM remains the cognitive authority: it supplies opaque, explicitly authorized
external route candidates and decides whether to use the returned recommendation.

Every recommendation records `simplicio.external-routing/v1`, dispatch ID, policy
(`interactive`, `background`, `review`, or `delivery`), signal names, selected opaque route/provider,
and a reason. It contains no prompt, context content, credential, or secret. Provider metrics carry
an explicit `measured`, `estimated`, `missing`, or `failed` state; absent pricing stays absent.

Budgets are scoped by organization/project/session/turn/stage/agent. Admission checks token and
context caps, quota, health, fan-out, backpressure, and an interactive reservation. External Loop
Hub remains responsible for the production queue and dispatch. `ContextPackRegistry` only creates
content-addressed metadata for already-produced Mapper/artifact bytes; it neither reads a workspace
nor duplicates the content. Reconciliation is exactly once per dispatch, and retry is refused while
an effect is unknown. Cancellation is free only while the tracked effect has not been reconciled.

The contract intentionally returns a denial instead of switching authority when external quota is
exhausted. Production provider, Loop Hub, Mapper, Runtime, 20-agent system, UI regression, and
quality-comparison evidence still require the corresponding external pinned components; unit tests
here are not final E2E approval.
