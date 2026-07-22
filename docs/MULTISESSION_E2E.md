# Multi-session E2E and benchmark harness

Issue #206's harness is an **evidence classifier**, not an agent or approval
authority. The already-authenticated external invoking LLM produces the trace;
the harness never starts an internal provider, local LLM, Runtime, Loop Hub, or
Orca. It also cannot grant the final E2E approval.

```bash
python3 scripts/multisession_e2e.py coordinator-trace.json --output receipt.json
```

The trace records all four surfaces, at least 20 stable sessions, observed
states, isolated worktrees, recovery/replay, governance separation, one
remotely re-queried delivery, and repeated fixed-fixture measurements. Metrics
without observations remain `null` with a reason. Missing credentials or an
installed dependency produces `BLOCKED`; violated invariants produce `FAILED`.
Only complete evidence becomes `READY_FOR_COORDINATOR_APPROVAL`, and even that
receipt always retains `final_e2e_approved: false` for the coordinator after
all dependent issue PRs reach `main`.

This PR intentionally does not execute the final scenario, approve it, merge a
PR, or close the issue. Raw traces must be sanitized before use and must never
contain tokens, email addresses, private setup material, or signed URLs.
