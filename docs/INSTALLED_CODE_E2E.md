# Installed AgentHost + Runtime Code E2E fixture

## Bounded install lifecycle harness

The offline lifecycle harness creates a new private prefix, copies and hashes
an old executable, observes `--version` and `probe`, canaries and atomically
upgrades to a new executable, then swaps back and proves the original digest
and observations. It never downloads or resolves `latest`, and it refuses a
pre-existing prefix, a missing/non-executable artifact, or a digest/command
mismatch. Run its deterministic, explicitly non-release fixture with:

```text
python3 scripts/release/installed_lifecycle_e2e.py --fixture \
  --prefix /tmp/simplicio-clean-install --output /tmp/lifecycle-receipt.json
```

Without `--fixture`, pass both immutable artifacts with `--old` and `--new`.
When neither is passed the runner prefers an actual `simplicio-code` found on
`PATH` (or `SIMPLICIO_CODE_INSTALLED_BIN`) and requires
`SIMPLICIO_CODE_UPGRADE_BIN`; it never substitutes a repository fixture.
Delete the chosen prefix before every run. The receipt omits host paths and
clocks so identical inputs produce identical bytes, records unavailable
platform/production evidence as `null` with reasons, and always leaves the
issue-closure claim false. Thus one local fixture run advances regression
coverage for #100/#57 but does **not** prove clean Windows/macOS installs,
publisher provenance, production rollout, or all acceptance criteria.

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
