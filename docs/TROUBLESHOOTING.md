# Troubleshooting — Simplicio Code

Covers the five areas called out in the onboarding issue: login, Runtime,
mapa (Mapper), rede (network), and updater. Each entry lists the symptom,
likely cause, and the fix — see [docs/QUICKSTART.md](QUICKSTART.md) for the
happy path and [docs/ARCHITECTURE.md](ARCHITECTURE.md) for how the pieces
fit together.

## Login

**Symptom:** `simplicio-code login` hangs or reports a device-authorization
error.

- This beta's login/entitlement sync (device authorization against the
  Simplicio gateway) is not wired up yet — see
  [docs/migration/legacy-login-migration.md](migration/legacy-login-migration.md).
  Until it lands, local development uses `OPENROUTER_API_KEY` set only in
  the environment (never in a config file, never in the binary).
- If you see an unrelated auth error, confirm the key is exported in the
  *same shell* that launches `simplicio-code` (`echo $OPENROUTER_API_KEY`),
  and that it hasn't been accidentally committed anywhere
  (`git grep -n OPENROUTER_API_KEY` should only match docs/code, never a
  literal key).

## Runtime

**Symptom:** the agent reports it cannot read files, or startup fails with a
Runtime handshake error.

- The Simplicio Runtime is fail-closed by design (see
  [docs/ARCHITECTURE.md](ARCHITECTURE.md#agent-runtime-e-code-como-um-produto)):
  if the Runtime process fails its identity/protocol handshake, the client
  refuses to fall back to direct disk reads rather than silently bypassing
  the sandbox.
- Confirm the Runtime binary is installed and discoverable (it ships coupled
  to the `simplicio-code` binary in this beta — reinstalling
  `simplicio-code` reinstalls it).
- Check that no other process is holding the same MCP stdio session for the
  workspace; only one Runtime session per workspace is expected.
- If the handshake fails transiently (e.g. the Runtime was still starting),
  the client retries by reconnecting the session — a persistent failure
  after a few seconds usually means the Runtime process itself did not
  start; check its logs before assuming the client is at fault.

## Mapa (Mapper)

**Symptom:** the project map never finishes, or context/search results look
stale.

- The Mapper runs in the background as soon as a folder is opened; on a
  very large repository the first pass can take longer than a small one —
  this is expected, not a hang.
- If the map appears stuck indefinitely (no progress after several
  minutes on a small-to-medium repo), restart `simplicio-code` in that
  folder — a fresh session forces a fresh Runtime connection and a fresh map
  pass.
- The Mapper is Runtime-owned specifically so the TUI, headless, and ACP
  entry points never duplicate map/memory/search state — if you see
  divergent results between two of those entry points for the same
  workspace, that's a bug worth reporting, not a config problem.

## Rede (network)

**Symptom:** unexpected outbound connections, or you want to confirm what
the client contacts before running it somewhere network-restricted.

- Run `simplicio-code privacy diagnose` to list every telemetry/crash-report
  destination and whether it's currently active — this makes no network
  calls itself.
- Telemetry is disabled by default; set `DO_NOT_TRACK=1` to force it off
  regardless of any other config (this is honored ahead of the first event —
  see the telemetry client in `crates/codegen/xai-grok-telemetry`).
- Inference/auth traffic (separate from telemetry) goes through the
  configured gateway/provider endpoints documented in
  [docs/ARCHITECTURE.md](ARCHITECTURE.md#gateway-simplicio) — those carry
  your prompts/code by necessity (that's the product function), unlike
  telemetry, which must never carry project content.

## Updater

**Symptom:** `simplicio-code update --check` fails, or you're not sure which
channel you're on.

```sh
simplicio-code update --check         # check without installing
simplicio-code update --check --json  # machine-readable
```

- Public signed installers/auto-update have not shipped yet for this beta
  (see [README.md#estado-do-produto](../README.md#estado-do-produto)); an
  update-check failure in this phase most often means there is no reachable
  release channel configured for your build, not a corrupted installation.
- `--alpha`/`--stable`/`--enterprise` select the release channel; switching
  channels does not itself trigger reinstall unless you also pass
  `--force-reinstall`.

## Still stuck?

Run `simplicio-code privacy diagnose` and re-read
[docs/ARCHITECTURE.md](ARCHITECTURE.md) — most "it doesn't work" reports
turn out to be one of: missing `protoc`/DotSlash for a from-source build
(see [docs/QUICKSTART.md](QUICKSTART.md#1-pré-requisitos)), an environment
variable not exported in the shell that launches the binary, or a
first-run Mapper pass that hasn't finished yet.
