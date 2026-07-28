# AGENTS.md — simplicio-code

## Simplicio Ecosystem Contract (canonical)

This repository is a Simplicio ecosystem component. For every non-trivial task: run `simplicio runtime map --repo . --for-llm markdown`, then `simplicio memory "<task>"`, rank/load relevant skills, execute through the native `simplicio` CLI, validate, and record evidence. MCP is fallback transport only.

### Component boundaries
Use `simplicio-mapper` for bounded context, `simplicio-fast` for snapshots and PlanDAG, `simplicio-dev-cli` for deterministic implementation, `simplicio-runtime` for contracts/gates/validation/receipts, `simplicio-loop` for convergence and close-gates, and `simplicio-agent` as control plane. Providers are workers, never authorities.

### Execution and evidence
Use `simplicio`/`simplicio shell compact` for inspection and `simplicio edit --plan` or governed dev-cli for mutation. Preserve `simplicio.io/v1`; run `simplicio contracts smoke --json`, focused tests and `simplicio validate "<task>" --repo . --json`; close only with `simplicio evidence`. Facts are `MEASURED|` only with receipts, otherwise `UNVERIFIED|`; savings come only from `simplicio savings report --repo . --json`. Missing dependencies fail closed; never fabricate context or output.
