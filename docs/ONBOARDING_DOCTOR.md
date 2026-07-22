# Reproducible agent-native onboarding

The pinned onboarding manifest is `config/onboarding-bundle-v1.json`. From a
clean checkout, run the read-only diagnostic first:

```bash
python3 scripts/onboarding_doctor.py --mode protocol_only
python3 scripts/onboarding_doctor.py --mode productive
```

The JSON report identifies each independent component's expected and detected
version, capabilities, origin, health and blocker. It also measures preflight
time and executable probes. `protocol_only` is deliberately diagnostic: its
`effect_authority` is always `false`. Productive work remains under the
external AgentHost/Runtime/Loop authority and requires their compatible
processes plus a secure persistent socket. Doctor never starts a provider,
local LLM, coordinator, Runtime, or socket, and it does not read credentials.

Install or upgrade missing layers using each component's pinned `origin`, one
at a time, outside Doctor. Review the package manager dry-run, obtain human
confirmation, preserve the active session, and keep its prior immutable pin
until reconnect succeeds. On failure, restore that pin and rerun Doctor; do
not kill an existing session. Attach/reconnect uses the existing
`SIMPLICIO_AGENT_SOCKET`, so a partial upgrade cannot silently create another
authority.

GitHub authentication is intentionally reported as `unknown`: use
`gh auth status` interactively rather than exposing tokens to a receipt.
macOS and Linux are supported by the first-run command above. Windows should
run it from a checkout with Python 3 and Git, but Unix-domain socket and file
permission results are environment-dependent; restricted containers may also
report blocked sockets or quota. No secret values belong in the manifest,
examples, reports, or receipts.
