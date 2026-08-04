# Current status

- Schema: `simplicio.code-status/v1`
- Generated: `2026-08-04T18:23:37.395592+00:00`
- Commit: `6a0ef57a20da4f4a8488f316083738e2ea88dc71`
- Dirty checkout: `False`

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
- Source revision: `main@5b0edd4aa974ac66f60a3579ce0d5c06e09ef574`
- Captured: `2026-08-04T18:22:00Z`

Statuses are evidence states, not closure claims; HISTORICAL_CLOSED preserves GitHub history without asserting current acceptance; refresh this inventory after live GitHub re-query.

| Issue | State | Priority | Owner | Dependencies | Evidence |
| --- | --- | --- | --- | --- | --- |
| #313 | `BLOCKED` | UNSET | Code + external hosts | Agent; Runtime; Loop | Real host A/B adoption evidence is required; local Code has no canonical UWP/SKILL surface. |
| #314 | `HISTORICAL_CLOSED` | UNSET | Code + external browser tooling | browser pane; Runtime | GitHub state is closed/completed after the Code-owned bounded loopback preview and file-url fail-fast evidence; the external Browser pane remains outside Code ownership. |
| #315 | `OPEN` | P0 | Code + Simplicio ecosystem | #317; #318; #319; #320; #321; #322; #323; #324; #325; #326; #327 | Parent recovery objective remains open until child ACs have merged evidence. |
| #316 | `HISTORICAL_CLOSED` | P0 | Code CI/toolchain | Rust 1.94.1; GitHub CI | GitHub state is closed/completed; PR #335 local receipt/cancellation hardening is merged, but full cargo check/test and CI/release evidence remain unverified. |
| #317 | `OPEN` | P0 | Runtime + Code | Runtime process capabilities; Agent | PRs #379, #393, #399 and #405 merged fail-closed runtime_process validation, idempotency-bound process receipts, cancel-input validation and zero-timeout exec rejection; productive Runtime/Loop Hub lifecycle evidence remains open. |
| #318 | `BLOCKED` | P0 | Code + Runtime | Runtime workspace contract; audit gate | PR #401 makes positive violation baselines fail closed and removes the known positive violation inventory; broad production-looking workspace accesses remain outside the manifest, so full migration and adversarial evidence are not proven. |
| #319 | `OPEN` | P0 | Code + Agent + Runtime + Loop | installed four-surface E2E | PRs #387 and #395 committed real external Loop Hub receipts covering four surfaces, single identity, restart/reconnect and PID rotation, including a commit-tied 10-run benchmark; installed diagnosis on 2026-08-04 returned BLOCKED with agent_host_missing, and the installed AgentHost/Runtime/Mapper/Fast version matrix, PTY/workspace/concurrency and release-grade cross-platform closure remain open. |
| #320 | `OPEN` | P0 | Code release | validated SHA; release assets; registry evidence | PR #386 now blocks accidental publication of historical beta.5; a new SHA-tied release with public assets, trust-root, install, update and rollback evidence remains open. |
| #321 | `OPEN` | P0 | Runtime installer + Code | trust root; Windows install/update | PR #380 merged actionable missing-OpenSSL blocking and PR #389 isolated Windows pytest/OpenSSL stdin; trust-root, Windows install/update/rollback and public release evidence remain open. |
| #322 | `OPEN` | P1 | Account/Gateway + Code | device login; entitlement backend | PRs #390 and #402 add fail-closed Pending/Expired session states, refresh recovery, and a Runtime-backed device-login completion path; the real device-auth/entitlement backend, production deployment, cross-OS keychain receipts and surface parity remain unverified. |
| #323 | `OPEN` | P1 | Gateway + Runtime + Code | streaming tools cancel | PRs #384 and #403 remove onboarding provider-key instructions, add a guard, and propagate validated request-id/idempotency headers; production gateway deployment with real streaming/tools/cancel receipts remains open. |
| #324 | `OPEN` | P1 | Code CI | headless matrix; calibrated invariants | PRs #381 and #400 merge deterministic 28-cell matrix regression coverage and a non-silent invariant-report lane; built-binary, TTY, permissions and cross-platform evidence remain open. |
| #325 | `OPEN` | P1 | Code + Runtime + Loop | cold/warm benchmark harness | PRs #392 and #395 merged Windows-native process/RSS/CPU sampling, output-directory recovery, and a commit-tied 10-run external Loop Hub receipt; S0-S3 Agent→Runtime→Loop matrix, installed AgentHost/Runtime proof, and Runtime issue-gate remain unverified. |
| #326 | `BLOCKED` | P1 | Agent UX + Code | workspace.observe/advisory contract; productive AgentHost | Source contracts and protocol-only tests exist, but productive AgentHost E2E and the full advisory receipt are unavailable. |
| #327 | `IN_PROGRESS` | P1 | Code governance | live GitHub re-query; version/capability sources | PRs #331/#332/#333/#334/#335/#337/#338/#339/#350/#351/#382/#383/#384/#386/#387/#388/#389/#390/#391/#392/#393/#395/#396/#397/#399/#400/#401/#402/#403/#405 merged status, quality, benchmark, installed-E2E, onboarding, external Loop receipt, release, auth-state, native sampling, receipt-integrity, Runtime process, workspace-gate, device-login, gateway-identity and commit-tied benchmark slices; live issue state and main SHA are refreshed here, while external release provenance/full CI and residual ownership evidence remain unverified. |

## Migration note

Current onboarding pins and their measured drift are documented in [`docs/migration/code-status-beta5.md`](../migration/code-status-beta5.md).
