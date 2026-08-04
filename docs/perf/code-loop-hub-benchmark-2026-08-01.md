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
| Code SHA | `68327f340883359a8b27d2decd054e3726d36033` |
| Loop SHA | `3b00cfadf5c519916526c71fc6a129f2de2e8e02` |
| Runs | `10` |
| Transport | `tcp://127.0.0.1:<ephemeral-port>` on Windows |
| Receipt SHA-256 | `D92BF111DA5508CE1C43DBCEAE53CCCB0AC113E8040488110E579C18DCE90A5B` |

## Measured metrics

| Metric | p50 (ms) | p95 (ms) |
| --- | ---: | ---: |
| Hub startup | 423.844 | 651.910 |
| Code client lifecycle test | 2,612.684 | 4,232.416 |
| Hub restart/reconnect downtime | 551.335 | 676.307 |

Every run reported one Hub identity, successful restart/reconnect, and the
lifecycle `handshake → attach → submit → progress → cancel → resume → replay`.
Every run reported `tui-1`, `tui-2`, `headless`, and `acp` surfaces. Provider and
local-LLM startup were both false.

## Explicit limits

Process count, RSS, and CPU remain `null`: this Windows runner does not expose the
Unix `ps`/`pgrep` probes, and the harness does not substitute zero or estimate
them. A future Windows process sampler is required before those metrics can
become gates. The restart cleanup now removes only the owned temporary lock
after the Hub process exits; all 10 restart/reconnect cycles completed. This
benchmark also does not claim AgentHost or Runtime effects; those remain
separate installed-E2E requirements.
