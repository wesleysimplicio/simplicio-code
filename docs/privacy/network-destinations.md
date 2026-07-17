# Network destination inventory (issue #13)

This is a full, concrete inventory of every network destination the
Simplicio Code client can contact — the first of the three acceptance
criteria left open after PR #27 ("full inventory of every network
destination", "schema allowlist enforcing only documented domains", "a test
that actually proxies/captures all outbound network traffic"). PR #27
already delivered `DO_NOT_TRACK` and `simplicio-code privacy diagnose`; this
document and the companion allowlist/test close the remaining gaps as far
as is realistically achievable in a single client-side slice (see
[What remains open](#what-remains-open) at the bottom — full-process
network capture is **not** claimed here).

Compiled by reading every crate that constructs an HTTP/WebSocket client or
hardcodes a URL: `xai-grok-telemetry`, `xai-mixpanel`, `xai-crash-handler`,
`xai-grok-env`, `xai-grok-shell` (`agent/config.rs`, `remote/*`,
`session/storage/*`, `upload/gcs.rs`), `xai-grok-update`,
`xai-grok-plugin-marketplace`, `xai-grok-agent` (plugins/git_install),
`xai-grok-sampler`, `xai-grok-tools` (grok_build implementations),
`xai-grok-config-types`, `simplicio-account-client`.

## Core product (not telemetry — carries prompts/code because that's the
product's function)

| Domain | Purpose | Source | Covered by DO_NOT_TRACK? |
|---|---|---|---|
| `cli-chat-proxy.grok.com` | Default chat/inference backend (session-token auth), subscription check, settings sync, subagent bundles, model catalog, managed-config, trace/session upload proxy | `crates/codegen/xai-grok-env/src/lib.rs:23` (`PRODUCTION_ENDPOINTS.cli_chat_proxy_base_url`), `crates/codegen/xai-grok-shell/src/agent/config.rs:46` (`CLI_CHAT_PROXY_BASE_URL_DEFAULT`), `crates/codegen/xai-grok-workspace/src/handle.rs:3740` | No — core function, not telemetry. Overridable via `GROK_CLI_CHAT_PROXY_BASE_URL` |
| `api.x.ai` | Direct xAI inference API for BYOK/direct API-key model configs | `crates/codegen/xai-grok-shell/src/agent/config.rs:48` (`XAI_API_BASE_URL_DEFAULT`), hit from `xai-grok-sampler/src/client.rs` | No — core function |
| `grok.com` (bare) | WS handshake `Origin` header | `crates/codegen/xai-grok-env/src/lib.rs:27` (`ws_origin`) | No |
| `code.grok.com` (`wss://code.grok.com/ws/code-agent`) | Web-frontend-driven agent relay | `crates/codegen/xai-grok-env/src/lib.rs:25` (`relay_ws_url`) | No |
| `grok.com` (`wss://grok.com/ws/gw/`) | `/cloud new` sandbox gateway | `crates/codegen/xai-grok-env/src/lib.rs:26` (`gateway_ws_url`) | No |
| `grok.com` (share links) | Builds `/build/share/{id}` share URLs | `crates/codegen/xai-grok-shell/src/remote/client.rs:10` (`GROK_CODE_WEB_URL`) | No |
| `grok.com` (conversation/mode/workspace sync) | `/rest/app-chat/conversations/{id}`, `/rest/modes`, workspace listing | `crates/codegen/xai-grok-shell/src/remote/conversations_client.rs:7`, `chat_models_client.rs:12`, `workspaces_client.rs:7` (`GROK_WEB_URL`) | No |
| `code.grok.com` (`https://code.grok.com`) | Session CRUD/hydration/share backend (`BackendClient`) | `crates/codegen/xai-grok-shell/src/remote/client.rs:8` (`GROK_CODE_BACKEND_URL`) | No |
| `assets.grok.com` | Static asset/CDN host | `crates/codegen/xai-grok-env/src/lib.rs:24` (`asset_server_url`), `xai-grok-shell/src/agent/config.rs:50` | No |
| `x.ai/cli` | Self-update: channel pointer + binary download; printed reinstall one-liners | `crates/codegen/xai-grok-update/src/version.rs:18` (`CLI_BASE_URL_PRIMARY`), `auto_update.rs:32,34` | No |
| `storage.googleapis.com` | Self-update download fallback (GCS); search-index download sync | `crates/codegen/xai-grok-update/src/version.rs:22-23` (`CLI_BASE_URL_FALLBACK`), `crates/codegen/xai-grok-shell/src/session/storage/search_remote_sync.rs:328` | No |
| `github.com` | Plugin marketplace entries (`"source": "github"`), `user/repo` plugin-install shorthand; GitHub Releases update path is resolved by the `gh` CLI, not dialed directly | `crates/codegen/xai-grok-plugin-marketplace/src/config.rs:136`, `crates/codegen/xai-grok-agent/src/plugins/git_install.rs:106`, `crates/codegen/xai-grok-update/src/version.rs:207` (`GH_RELEASE_REPO`) | No |
| npm registry (operator-configurable, no hardcoded default) | Self-update via `npm view`/`npm install` (`NPM_PACKAGE = "@xai-official/grok"`) | `crates/codegen/xai-grok-update/src/version.rs:13` | No |
| `console.cloud.google.com` | Link text only (e.g. for Slack) — not fetched by the client | `crates/codegen/xai-grok-shell/src/upload/gcs.rs:139` | N/A — not a request |

## Telemetry / analytics / crash-reporting

| Domain | Purpose | Default | Source | Covered by DO_NOT_TRACK? |
|---|---|---|---|---|
| `cli-chat-proxy.grok.com` | First-party product-usage events endpoint (`events_url`); first-party OTLP session-metrics trace upload | Off | `crates/codegen/xai-grok-telemetry/src/config.rs:97,131-132` (`events_url`, env `GROK_TELEMETRY_EVENTS_URL`/build-time `GROK_TELEMETRY_BUILD_EVENTS_URL`); POST in `client.rs:226-233`; traces in `otel_layer/mod.rs` | **Yes** — `TelemetryMode`/`is_telemetry_disabled_sync` gate, `DO_NOT_TRACK` added in PR #27 |
| `api.mixpanel.com` | Legacy product analytics (`track`/`engage`) | Off | `crates/codegen/xai-mixpanel/src/lib.rs` (`DEFAULT_BASE_URL`, this PR); gated by `mixpanel_enabled`+`mixpanel_token` in `xai-grok-telemetry/src/client.rs:78-88` | **Yes** — same mode gate + `DO_NOT_TRACK` |
| Sentry (dynamic DSN host, no compiled-in literal) | Crash/error reports, PII-scrubbed before send | Off unless a DSN is baked in and mode ≥ `session_metrics` | `crates/codegen/xai-grok-telemetry/src/sentry.rs:39-58` (`option_env!("SENTRY_DSN")`) | **Yes** — same gate |
| Customer-configured OTLP endpoint (`OTEL_EXPORTER_OTLP_ENDPOINT` etc., default `http://localhost:4318`/`4317` per OTLP spec, not an xAI host) | Bring-your-own observability, double opt-in | Off | `crates/codegen/xai-grok-telemetry/src/external/config.rs:216,218` | Partially — gated by `GROK_EXTERNAL_OTEL`; `DO_NOT_TRACK` does not need to touch it since it's already off by default and customer-controlled |

## No network destination at all

- **`xai-crash-handler`** — fully custom, disk-only crash reporter (Unix
  `sigaction` / Windows `SetUnhandledExceptionFilter`). Writes locally to
  `history/crash-<ts>.txt`; no `reqwest`/socket code anywhere
  (`crates/codegen/xai-crash-handler/src/handler.rs:441-479,752-798`,
  `lib.rs:117-144`). This is a **separate crate from Sentry**, not a
  network-facing "crash handler" destination.
- **`simplicio-account-client`** — the crate's own doc comment states it
  performs zero network I/O; the only `https://simplicio.dev/device`-shaped
  strings in the crate are test-only JSON fixtures
  (`crates/codegen/simplicio-account-client/src/schema.rs:181-182`).

## Excluded as noise (not real destinations)

Doc-comment/example links (`docs.rs`, `json-schema.org`, `dotslash-cli.com`,
`invisible-island.net`); `example.com`/`*.example`/`*.invalid`-shaped test
fixtures (`auth.example.com`, `npm.example.com`,
`proxy.corp.example.com`, MCP header-heuristic test hosts like
`mcp.figma.com`/`mcp.linear.app`/`mcp.sentry.dev`); `localhost`/`127.0.0.1`
mock servers used by the test suite (wiremock/mockito/axum); the test-only
`x.ai/grok` literal in `xai-grok-announcements/src/lib.rs:306,311`;
`auth.x.ai` (doc-comment example OAuth2 issuer only,
`xai-grok-config-types/src/lib.rs:294,302` — never a literal runtime
string); `accounts.x.ai` does not appear anywhere in the repository.

## Schema allowlist (telemetry/crash-report scope)

A machine-checkable allowlist for the **telemetry/analytics** destinations
(not the whole product surface above — see rationale in the module doc
comment) lives in
[`crates/codegen/xai-grok-telemetry/src/allowlist.rs`](../../crates/codegen/xai-grok-telemetry/src/allowlist.rs):

```rust
pub const ALLOWED_TELEMETRY_HOSTS: &[&str] =
    &["api.mixpanel.com", "cli-chat-proxy.grok.com"];
```

`assert_allowed_telemetry_url(url)` fails closed on an unparsable URL, a
URL with no host, or a host not in the list (exact string match — no
suffix/prefix matching, so `api.mixpanel.com.evil.example` is rejected).
Sentry (dynamic DSN) and external customer OTEL (deliberately
customer-configured) are documented exceptions, not silently ignored gaps —
see the module doc comment for why they can't be pinned to a fixed host
string.

## Network-capture test

[`crates/codegen/xai-grok-telemetry/tests/network_destination_allowlist.rs`](../../crates/codegen/xai-grok-telemetry/tests/network_destination_allowlist.rs)
spins up a real local HTTP server on loopback (`127.0.0.1:0`), redirects
**both** telemetry sinks this crate owns (product-events `events_url` and
Mixpanel, via the new `mixpanel_base_url` config override) at that server,
drives a representative telemetry operation (`client::init` +
`log_event(ManualAuth)`, which together fire `/events`, `/track`, and
`/engage`), and asserts:

1. Exactly those three documented paths were hit — a catch-all fallback
   route fails the test if anything else lands on the server.
2. The production defaults (no override) resolve to hosts that are members
   of `ALLOWED_TELEMETRY_HOSTS`.

Run it: `cargo test -p xai-grok-telemetry --test network_destination_allowlist`

## What remains open

Being explicit about what this PR does **not** close, per the original
issue #13 acceptance criteria:

- **This is not a full-process MITM/proxy capture.** It proves the two
  telemetry sinks this crate owns only contact their configured host for a
  representative operation; it does not spawn the *built binary* and
  intercept literally every socket the whole process opens (that would also
  have to cover the core-product destinations in the first table, which are
  expected to fire — the interesting assertion there is "at least this set
  of documented hosts, nothing extra," which needs a real system-level
  proxy harness and CI plumbing beyond this crate's test suite). Tracked as
  follow-up.
- **Sentry and external-OTEL hosts are not in the machine-checked
  allowlist** because their host isn't a fixed compile-time string (build/env
  supplied). This is a documented, deliberate limitation, not an oversight.
- **Backend retention/deletion enforcement** for first-party events remains
  a server-side commitment outside this repository (unchanged from PR #27's
  `docs/privacy/telemetry.md` §5).
- **Mixpanel removal** is still flagged as deprecated, not removed (unchanged
  from PR #27).
