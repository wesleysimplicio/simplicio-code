# Current status

- Schema: `simplicio.code-status/v1`
- Generated: `2026-08-04T05:53:01.057280+00:00`
- Commit: `65e9345d605c0d84e042396c32aace61afe25851`
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
- Source revision: `main@65e9345d605c0d84e042396c32aace61afe25851`
- Captured: `2026-08-04T05:52:10.8487391Z`

Statuses are evidence states, not closure claims; refresh this inventory after live GitHub re-query.

| Issue | State | Priority | Owner | Dependencies | Evidence |
| --- | --- | --- | --- | --- | --- |
| #313 | `BLOCKED` | UNSET | Code + external hosts | Agent; Runtime; Loop | Real host A/B adoption evidence is required; local Code has no canonical UWP/SKILL surface. |
| #314 | `BLOCKED` | UNSET | Code + external browser tooling | browser pane; Runtime | Bounded loopback preview merged; external browser verification remains unavailable and issue is still open. |
| #315 | `OPEN` | P0 | Code + Simplicio ecosystem | #317; #318; #319; #320; #321; #322; #323; #324; #325; #326; #327 | Parent recovery objective remains open until child ACs have merged evidence. |
| #316 | `BLOCKED` | P0 | Code CI/toolchain | Rust 1.94.1; GitHub CI | PR #335 merged local receipt/cancellation hardening; full cargo check/test timed out and CI/release evidence remain unverified. |
| #317 | `OPEN` | P0 | Runtime + Code | Runtime process capabilities; Agent | Foreground/background/cancel/reconcile contracts require Runtime-owned capabilities and installed E2E. |
| #318 | `BLOCKED` | P0 | Code + Runtime | Runtime workspace contract; audit gate | Scoped audit passes, but broad production-looking workspace accesses remain outside the manifest; full migration and adversarial evidence are not proven. |
| #319 | `OPEN` | P0 | Code + Agent + Runtime + Loop | installed four-surface E2E | Installed AgentHost/Runtime/Loop Hub proof is not yet a merged, four-surface receipt. |
| #320 | `OPEN` | P0 | Code release | validated SHA; release assets; registry evidence | Replacing beta.5 requires a new SHA-tied release and public asset evidence. |
| #321 | `OPEN` | P0 | Runtime installer + Code | trust root; Windows install/update | Productive trust-root and Windows install/update/rollback evidence remains open. |
| #322 | `OPEN` | P1 | Account/Gateway + Code | device login; entitlement backend | Device login and renewable entitlement require backend-backed proof. |
| #323 | `OPEN` | P1 | Gateway + Runtime + Code | streaming tools cancel | OpenRouter replacement requires a governed gateway with real streaming/tools/cancel evidence. |
| #324 | `OPEN` | P1 | Code CI | headless matrix; calibrated invariants | Headless matrix promotion to blocking gates remains open. |
| #325 | `OPEN` | P1 | Code + Runtime + Loop | cold/warm benchmark harness | Startup/workspace-open/first-effect measurements and regression limits remain open. |
| #326 | `BLOCKED` | P1 | Agent UX + Code | workspace.observe/advisory contract; productive AgentHost | Source contracts and protocol-only tests exist, but productive AgentHost E2E and the full advisory receipt are unavailable. |
| #327 | `IN_PROGRESS` | P1 | Code governance | live GitHub re-query; version/capability sources | Status/inventory gate merged in PRs #331/#332, snapshot anchor corrected in #333, live source refreshed in #334, and PR #335 validation evidence recorded; external release and full-CI evidence remain unverified. |

## Migration note

Current onboarding pins and their measured drift are documented in [`docs/migration/code-status-beta5.md`](../migration/code-status-beta5.md).
