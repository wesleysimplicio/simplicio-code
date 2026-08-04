# Status migration note: stale beta.5 onboarding pin

## Measured current state

The repository source manifests and the installed local probes no longer agree with the historical onboarding bundle that pinned Code beta.5, Mapper 0.26.0, Dev CLI 0.18.0, and Loop Hub 3.38.6.

The refreshed bundle records Code `0.3.0-beta.3`, Simplicio Runtime `3.6.0`, Mapper `0.26.15`, Dev CLI `0.18.6`, and Loop Hub `3.38.35`. Code's source version is derived from `pyproject.toml`; the other values are captured probe observations, not release publication claims.

## Migration rule

Consumers must not treat the bundle as proof of a public release. Before productive adoption:

1. re-query the component probes;
2. verify immutable commit/tag and artifact digests;
3. require compatible AgentHost, Runtime, and Loop Hub handshakes;
4. keep protocol-only fixtures separate from installed E2E evidence;
5. fail closed when a productive dependency is missing, incompatible, or not proven.

The previous beta.5 value is retained only as historical drift context. It is not a current Code release claim.

## Residuals

AgentHost remains protocol-pinned but productive availability is unverified. Release publication, cross-platform packaging, and the remaining open issues in `docs/status/residual-issues.v1.json` require independent evidence before closure.