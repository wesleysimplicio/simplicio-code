# Code → Loop Hub benchmark

Status: `MEASURED| external Loop Hub daemon`

This report measures the existing Code client against an independently checked
out Loop Hub daemon. Code did not start a scheduler, Runtime, Mapper, model, or
LLM. It covers the four surfaces represented by the external contract and the
Hub lifecycle; it is not proof of the separately required AgentHost/Runtime
installation or of a production release.

## Reproduction

```powershell
python scripts/code_loop_hub_e2e.py `
  --repo . `
  --loop-root ..\simplicio-loop `
  --output dist\code-loop-hub-benchmark-10.json `
  --runs 10
```

Observed environment:

| Input | Value |
| --- | --- |
| Code SHA | `39ebdd93872b8fae0dc412641acc94c1c7bdc980` |
| Loop SHA | `debe22e2d693a3042d09b6d452c312e7713fc8cf` |
| Runs | `10` |
| Transport | `tcp://127.0.0.1:<ephemeral-port>` on Windows |
| Receipt SHA-256 | `3CAE54C03081A0280F093F689A86FBC35CE0D9E6D2DDF37DDA8A30FDF32323A0` |

## Measured metrics

| Metric | p50 (ms) | p95 (ms) |
| --- | ---: | ---: |
| Hub startup | 402.401 | 421.340 |
| Code client lifecycle test | 1,441.232 | 1,810.371 |
| Hub restart/reconnect downtime | 417.947 | 431.858 |

Every run reported one Hub identity, successful restart/reconnect, and the
lifecycle `handshake → attach → submit → progress → cancel → resume → replay`.
Every run reported `tui-1`, `tui-2`, `headless`, and `acp` surfaces. Provider and
local-LLM startup were both false.

## Explicit limits

Process count, RSS, and CPU are `null`: this Windows runner does not expose the
Unix `ps`/`pgrep` probes, and the harness does not substitute zero or estimate
them. A future Windows process sampler is required before those metrics can
become gates. This benchmark also does not claim AgentHost or Runtime effects;
those remain separate installed-E2E requirements.
