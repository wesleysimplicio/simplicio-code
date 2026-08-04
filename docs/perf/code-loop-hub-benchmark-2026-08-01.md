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
| Code SHA | `b9aae7035e8e7cb6cb6a8c7a0e532ce5b779dd5a` |
| Loop SHA | `a0f9cbe886346db0a40a893fba6a9710f6121e1c` |
| Runs | `10` |
| Transport | `tcp://127.0.0.1:<ephemeral-port>` on Windows |
| Receipt | [`docs/evidence/issue-325-code-loop-hub-10.json`](../evidence/issue-325-code-loop-hub-10.json) |
| Receipt SHA-256 | `839dd8bb7c5e0f8d463a60d1f64da96ffc58d35ada9092582fd4c0222b3e3445` |

## Measured metrics

| Metric | p50 (ms) | p95 (ms) |
| --- | ---: | ---: |
| Hub startup | 438.532 | 559.324 |
| Code client lifecycle test | 2,938.487 | 3,275.900 |
| Hub restart/reconnect downtime | 543.725 | 571.854 |

Every run reported one Hub identity, successful restart/reconnect, and the
lifecycle `handshake → attach → submit → progress → cancel → resume → replay`.
Every run reported `tui-1`, `tui-2`, `headless`, and `acp` surfaces. The measured
maximums were one Hub process, 38,072 KiB RSS, and 8.965% CPU. Provider and
local-LLM startup were both false; Code did not start Runtime, Mapper, or the
scheduler.

## Explicit limits

The Windows-native sampler used Toolhelp32, GetProcessMemoryInfo, and
GetProcessTimes for this receipt; unavailable observations remain `null` rather
than zero or estimates. The restart cleanup removes only the owned temporary
lock after the Hub process exits; all 10 restart/reconnect cycles completed.
The receipt still does not prove the separately required installed
AgentHost/Runtime/Loop matrix or a production release. This
benchmark also does not claim AgentHost or Runtime effects; those remain
separate installed-E2E requirements.
