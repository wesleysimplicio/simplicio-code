# Issue 55 resilience audit

## Audited boundary

Current Code already negotiates the merged `simplicio.loop-hub-client/v1` and
`simplicio.loop-hub/v1` contracts, attaches four logical surface sessions to
one external Hub identity, and keeps Runtime, Mapper, scheduler, inference,
claims, and queues Hub-owned. The concrete slice in this change is restart
recovery for a safe progress read: the external daemon process is terminated,
restarted at the same endpoint, and Code reconnects with the last causal
cursor. Submit, cancel, and resume remain deliberately non-replayed when their
outcome is unknown.

## Reproducible receipt

Run from a clean checkout with a separate checkout of `simplicio-loop`:

```sh
python scripts/code_loop_hub_e2e.py --repo . --loop-root ../simplicio-loop \
  --runs 3 --output /tmp/issue-55-restart-receipt.json
```

The command starts only the external Loop daemon. The Rust client writes a
rendezvous marker after observing progress; the harness rotates the daemon PID,
waits for its socket, then permits the next progress read. A passing receipt
requires `restart_reconnected`, `hub_pid_rotated`, one shared Hub identity, and
provider-free flags. Timings are measured from monotonic clocks; p95 is `null`
for a single run rather than estimated.

## Residual acceptance criteria

Issue 55 remains open. This slice does **not** prove Runtime or Mapper restart,
fair interactive reservation without background starvation, installed TUI/
headless/ACP binaries, multiple workspaces/worktrees, cross-platform named
pipes, cancel during a real effect, or end-to-end goal → diff → test → remotely
confirmed PR. It also does not claim workload token/cost or first-token metrics,
because the proof is intentionally provider-free. Those need external
ecosystem E2E receipts before the issue can close.
