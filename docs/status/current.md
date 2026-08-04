# Current status

- Schema: `simplicio.code-status/v1`
- Generated: `2026-08-04T14:47:55.832270+00:00`
- Commit: `a8b5cce9ca0bd04c88d0a4f56de5dccf457f2666`
- Dirty checkout: `True`

## Version sources

| Source | Version |
| --- | --- |
| python | `0.3.0b3` |
| rust | `0.3.0-beta.3` |
| readme | `0.3.0-beta.3` |
| onboarding_bundle | `0.3.0-beta.3` |

- Source version status: `PASS`
- Release evidence status: `UNKNOWN`
- Exact tag: `null`
- Release evidence reason: release provenance is not proven by this checkout

This file is evidence for the current checkout, not proof of a published release.

## Capability inventory

Derived from source presence; external evidence is listed separately and is not inferred as PASS.

| Capability | Source status | External evidence |
| --- | --- | --- |
| workspace-access-audit | `implemented` | `local-audit-required` |
| runtime-exec/v1 | `implemented` | `installed-runtime-required` |
| runtime-process-lifecycle/v1 | `implemented` | `installed-runtime-hub-required` |
| agent-host/v1 | `implemented` | `installed-agent-host-required` |
| workspace.observe/v1 | `implemented` | `agent-host-emission-required` |
| simplicio-gateway/v1 | `contract-client-only` | `production-wiring-and-backend-required` |
| simplicio-account-device-auth/v1 | `contract-client-only` | `production-wiring-and-backend-required` |

## Residual issue inventory

- Source: GitHub live issue list for wesleysimplicio/simplicio-code
- Source revision: `main@a8b5cce9ca0bd04c88d0a4f56de5dccf457f2666`
- Captured: `2026-08-04T14:47:27.3517482Z`

Statuses are evidence states, not closure claims; HISTORICAL_CLOSED preserves GitHub history without asserting current acceptance; refresh this inventory after live GitHub re-query.

| Issue | State | Priority | Owner | Dependencies | Evidence |
| --- | --- | --- | --- | --- | --- |
| #313 | `BLOCKED` | UNSET | Code + external hosts | Agent; Runtime; Loop | Real host A/B adoption evidence is required; local Code has no canonical UWP/SKILL surface. |
| #314 | `BLOCKED` | UNSET | Code + external browser tooling | browser pane; Runtime | PR #378 merged bounded URL/duration regression coverage; external browser verification remains unavailable and issue is still open. |
| #315 | `OPEN` | P0 | Code + Simplicio ecosystem | #317; #318; #319; #320; #321; #322; #323; #324; #325; #326; #327 | Parent recovery objective remains open until child ACs have merged evidence. |
| #316 | `HISTORICAL_CLOSED` | P0 | Code CI/toolchain | Rust 1.94.1; GitHub CI | GitHub state is closed/completed; PR #335 local receipt/cancellation hardening is merged, but full cargo check/test and CI/release evidence remain unverified. |
| #317 | `OPEN` | P0 | Runtime + Code | Runtime process capabilities; Agent | PR #379 merged fail-closed runtime_process capability validation; productive Runtime/Loop Hub lifecycle evidence remains open. |
| #318 | `BLOCKED` | P0 | Code + Runtime | Runtime workspace contract; audit gate | Scoped audit passes, but broad production-looking workspace accesses remain outside the manifest; full migration and adversarial evidence are not proven. |
| #319 | `OPEN` | P0 | Code + Agent + Runtime + Loop | installed four-surface E2E | PRs #345-#351 record installed dependency, AgentHost, Runtime 3.6.0 and restart slices; external Loop Hub E2E now passed four surfaces with restart/reconnect, but the receipt is not yet committed and PTY/workspace/concurrency/version matrix closure remains open. |
| #320 | `OPEN` | P0 | Code release | validated SHA; release assets; registry evidence | Replacing beta.5 requires a new SHA-tied release and public asset evidence. |
| #321 | `OPEN` | P0 | Runtime installer + Code | trust root; Windows install/update | PR #380 merged actionable missing-OpenSSL blocking; trust-root, Windows install/update/rollback and public release evidence remain open. |
| #322 | `OPEN` | P1 | Account/Gateway + Code | device login; entitlement backend | Device login and renewable entitlement require backend-backed proof. |
| #323 | `OPEN` | P1 | Gateway + Runtime + Code | streaming tools cancel | OpenRouter replacement requires a governed gateway with real streaming/tools/cancel evidence. |
| #324 | `OPEN` | P1 | Code CI | headless matrix; calibrated invariants | PR #381 merged deterministic 28-cell matrix regression coverage; built-binary, TTY, permissions and cross-platform evidence remain open. |
| #325 | `OPEN` | P1 | Code + Runtime + Loop | cold/warm benchmark harness | PR #339 merged the 10-run Loop Hub restart benchmark and cleanup fix; S0-S3 Agent→Runtime→Loop matrix, installed AgentHost/Runtime proof, Windows process/RSS/CPU sampling, and Runtime issue-gate remain unverified. |
| #326 | `BLOCKED` | P1 | Agent UX + Code | workspace.observe/advisory contract; productive AgentHost | Source contracts and protocol-only tests exist, but productive AgentHost E2E and the full advisory receipt are unavailable. |
| #327 | `IN_PROGRESS` | P1 | Code governance | live GitHub re-query; version/capability sources | PRs #331/#332/#333/#334/#335/#337/#338/#339/#350/#351 merged status, quality, benchmark and installed-E2E slices; live issue state and main SHA are refreshed here, while external release provenance/full CI and residual ownership evidence remain unverified. |

## Migration note

Current onboarding pins and their measured drift are documented in [`docs/migration/code-status-beta5.md`](../migration/code-status-beta5.md).
