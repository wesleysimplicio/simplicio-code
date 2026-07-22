# Installed AgentHost + Runtime Code E2E fixture

Run the repository-owned, no-network fixture from the repository root:

```console
python3 scripts/installed_code_e2e.py --output installed-e2e-receipt.json
```

The command installs a private copy of the fixture into a temporary `bin`
directory, starts an AgentHost Unix socket and a Runtime MCP stdio process,
then removes the complete installation. It exercises `host.status`, stable
causal identity, turns from the TUI, headless, ACP, and workspace entry-point
profiles, cancellation/reconciliation, deterministic restart and advisory replay,
Runtime atomic edit, and argv-safe execution. The JSON receipt includes the
fixture SHA-256, a deterministic `evidence_sha256` over all non-timing
evidence, the requested atomic/rollback policy and resulting effect state,
and measured fixture throughput. Repeating the command must produce the same
`evidence_sha256` even though elapsed time differs. Unobservable production
latency is explicitly `null`-equivalent with a reason.

This fixture is external to Code's productive process, but it is **not** a
replacement implementation of AgentHost or Runtime. It refuses to start
without the runner's private `SIMPLICIO_CODE_E2E_FIXTURE=1` opt-in, is never
discovered by the normal product paths, and grants no production authority.
Use independently released executors for release acceptance evidence.

The regression suite is also standalone and requires no paid Actions:

```console
python3 -m unittest scripts.tests.test_installed_code_e2e
```

CI runs this command in the secret-free `simplicio-owned-reports` job and
uploads the receipt. The suite includes a missing-capability failure injection,
invalid causal identity and path traversal rejection, deterministic replay,
restart/reconnect, cancellation/reconciliation, and two-run evidence comparison.
