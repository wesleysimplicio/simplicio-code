# Installed AgentHost + Runtime Code E2E fixture

Run against independently installed executors from the repository root:

```console
SIMPLICIO_AGENT_HOST_E2E_COMMAND='["/opt/simplicio-agent","serve","--socket","{socket}"]' \
SIMPLICIO_RUNTIME_BIN=/opt/simplicio \
python3 scripts/installed_code_e2e.py --output installed-e2e-receipt.json
```

The AgentHost command is a JSON argv array, not a shell command; `{socket}` is
replaced with a private socket path. Missing, non-executable, or incompatible
dependencies fail closed. Only this mode can emit `proof_kind:
external_installed`.

When no model provider is configured, the independently shipped AgentHost can
run its explicit `--deterministic-e2e` mode. That mode still exercises the real
AgentHost session, causal identity, cancellation/reconciliation, restart and
advisory replay boundaries, but performs zero provider calls. It is valid
evidence for Code↔AgentHost↔Runtime transport/effect integration; it is not
evidence of model quality or provider availability. Runtime remains
provider/model-neutral and does not embed a local LLM.

The harness starts an AgentHost Unix socket and a Runtime MCP stdio process,
then removes the temporary workspace. It exercises `host.status`, stable
causal identity, turns from the TUI, headless, ACP, and workspace entry-point
profiles, cancellation/reconciliation, deterministic restart and advisory replay,
Runtime atomic edit, and argv-safe execution. Runtime compatibility is proven
before the first Agent turn. Missing and incompatible AgentHost/Runtime cases
are then exercised for every surface and recorded with `effect_attempted: false`,
proving that dependency failure cannot silently become a productive turn.

The repository-owned no-network fixture remains available for regression tests:

```console
python3 scripts/installed_code_e2e.py --fixture --output fixture-receipt.json
```

This fixture is external to Code's productive process, but it is **not** a
replacement implementation of AgentHost or Runtime. It refuses to start
without the runner's private `SIMPLICIO_CODE_E2E_FIXTURE=1` opt-in, is never
discovered by the normal product paths, and grants no production authority.
Its receipt is permanently labelled `hermetic_fixture_non_proof`, includes its
SHA-256, and records unavailable production latency explicitly. Use
independently released executors for release acceptance evidence.

The regression suite is also standalone and requires no paid Actions:

```console
python3 -m unittest scripts.tests.test_installed_code_e2e
```
To exercise the independently installed product binaries rather than the
hermetic contract fixture, pass the installed launcher explicitly:

```bash
python3 scripts/installed_code_e2e.py --installed /path/to/simplicio
```

This mode is deliberately fail-closed: the executable must exist and advertise
`simplicio_fs_list`, `simplicio_fs_stat`, `simplicio_edit`, and
`simplicio_exec`. The E2E edits a file, observes it through list/stat, then
executes an argv-safe command and requires an authoritative `completed` effect
receipt. It never substitutes the fixture when the installed binary is absent
or incompatible.
