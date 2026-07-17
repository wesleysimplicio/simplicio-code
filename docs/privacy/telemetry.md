# Telemetry and data controls (issue #13)

This documents the **current, real** telemetry surface of Simplicio Code —
what exists in this tree today (`crates/codegen/xai-grok-telemetry`,
`xai-mixpanel`, `xai-crash-handler`) — plus the two controls added by this
change (`DO_NOT_TRACK` support and `simplicio-code privacy diagnose`). It is
not a rewrite of the telemetry engine; the engine was already fairly mature
(mode gating, Sentry scrubbing, fail-closed OTel redaction). This is the
inventory, allowlist contract, and the two closable gaps identified against
the acceptance criteria.

## 1. Inventory: what can leave the machine, and how

| Destination | Purpose | Default | Gate |
|---|---|---|---|
| First-party events endpoint (`events_url`) | Aggregate product usage events | Off | `TelemetryMode` (`[features] telemetry`, `GROK_TELEMETRY_ENABLED`, `DO_NOT_TRACK`) |
| Mixpanel (`api.mixpanel.com`) | **Legacy** product analytics | Off, and should be treated as deprecated | `mixpanel_enabled` + `mixpanel_token`, same mode gate |
| Sentry (build-time DSN) | Crash/error reports, PII-scrubbed | Off unless mode ≥ `session_metrics` | Same mode gate; `crates/codegen/xai-grok-telemetry/src/sentry.rs` scrubs home dir, usernames, secrets before send |
| Trace/turn upload (`cli-chat-proxy.grok.com`, GCS-backed) | Session trace upload for debugging | Off by default, opt-in beyond base telemetry | `trace_upload` config / `GROK_TELEMETRY_TRACE_UPLOAD` |
| External OpenTelemetry (customer-configured) | Customer's own observability backend | Off, **double opt-in** | `GROK_EXTERNAL_OTEL` master switch **and** an explicit exporter (`otel_metrics_exporter`/`otel_logs_exporter`); content gates `otel_log_user_prompts`/`otel_log_tool_details` default `false` |

Inference/auth endpoints (`api.x.ai`, `auth.x.ai`, `accounts.x.ai`,
`grok.com`, `assets.grok.com`) are **not telemetry** — they carry your
prompts/code because that is the product's function, documented separately
in [docs/ARCHITECTURE.md](../ARCHITECTURE.md). Telemetry, by definition in
this document, must never carry project content; see §2.

For the full, file:line-cited inventory of every network destination the
client can contact (core product **and** telemetry), plus the
machine-checked telemetry-scoped allowlist and the network-capture test
that exercises it, see
[docs/privacy/network-destinations.md](network-destinations.md).

## 2. Allowlist: fields that never leave via telemetry

Regardless of destination or mode, the following are excluded from every
telemetry event (issue #13 step 4: "excluir prompts, respostas, conteúdo,
nomes de arquivos e caminhos completos"):

- `prompt_text`, `completion_text` — full prompt/response content
- `file_contents` — any file body
- absolute or project-relative `file_path` / `file_name`
- `tool_call_arguments`, `tool_call_output`
- `raw_request_body`, `raw_response_body`

This list is not just prose — it is a live, tested constant
(`NEVER_SENT_FIELDS` in
`crates/codegen/xai-grok-telemetry/src/diagnostic_report.rs`) that
`simplicio-code privacy diagnose` prints on every run, so the list and the
tool that reports it can't silently drift apart.

The external-OTel exporter additionally applies fail-closed schema
validation (`crates/codegen/xai-grok-telemetry/src/external/redact.rs`):
records with any non-allowlisted key are **dropped**, not forwarded with
the unknown key stripped — a validator bug fails safe.

## 3. Opt-out

Two independent mechanisms, so a user only needs to know one of them:

1. **`DO_NOT_TRACK=1`** (or any non-empty value other than `0`/`false`/`no`/`off`)
   — the [community convention](https://consoledonottrack.com/), added by
   this change. Checked in `xai_grok_telemetry::config::do_not_track_requested()`
   and wired into every telemetry gate that existed before this change:
   `Config::resolve_telemetry_mode` (async path, used once the config is
   loaded) and `is_telemetry_disabled_sync`/`is_telemetry_explicitly_disabled_sync`
   (sync path, used by `init_sentry`/OTel setup that runs *before* the async
   config is available — this is what makes "opt-out é respeitado antes do
   primeiro evento" true even for the earliest-initialized subsystems).
   It overrides `GROK_TELEMETRY_ENABLED=1` and any config-file setting; only
   an enterprise `Requirement` pin (managed config) sits above it.
2. **Product-specific**: `GROK_TELEMETRY_ENABLED=0`, `[features] telemetry
   = false` in `config.toml`, or the in-session `/privacy opt-out` (coding
   data sharing) / `/privacy` (status) slash command
   (`crates/codegen/xai-grok-pager/src/slash/commands/privacy.rs`).

## 4. "What would be sent" — `simplicio-code privacy diagnose`

New in this change (`crates/codegen/xai-grok-pager/src/privacy_cmd.rs`,
built on the pure `build_diagnostic_report` function in
`crates/codegen/xai-grok-telemetry/src/diagnostic_report.rs`). Run:

```sh
simplicio-code privacy diagnose         # human-readable
simplicio-code privacy diagnose --json  # machine-readable
```

It never makes a network call and never emits a telemetry event itself — it
only inspects the resolved `TelemetryMode` and `TelemetryConfig` and reports,
per destination: whether it's currently active, and why (which config
layer decided that). This directly satisfies "usuário consegue listar o que
seria enviado."

## 5. Pseudonymization, rotation, retention, deletion

- `deployment_id_from_key` (`crates/codegen/xai-grok-telemetry/src/config.rs`)
  derives a stable UUIDv5 from a deployment key rather than sending the raw
  key — already in place.
- Retention/deletion at the backend (server-side data lifecycle for
  first-party events) is **out of scope for this client-side change** — it
  is a backend/infra commitment, not something this repository's code can
  enforce or prove. Flagging this explicitly rather than claiming it's done:
  the acceptance criterion "retenção e exclusão são aplicadas no backend"
  needs a corresponding change in the events-ingestion service, tracked
  separately from this client PR.
- Legacy-destination removal (Mixpanel) is flagged as deprecated in §1 but
  intentionally **not deleted** in this change — removing a working
  destination outright without prior notice/rollout is a bigger, riskier
  change than the scope taken on here; it should be a follow-up once a
  first-party replacement is confirmed to have parity.

## 6. What was verified

- `cargo test -p xai-grok-telemetry --lib` — **157 tests passing**, including
  the new `diagnostic_report::tests::*` (7 tests) and
  `config::tests::do_not_track_*` (2 tests).
- `cargo test -p xai-grok-shell --lib do_not_track` — of the two new
  DO_NOT_TRACK precedence tests, `do_not_track_overrides_explicit_enable_in_sync_gates`
  **passed**. `do_not_track_overrides_resolve_telemetry_mode` could **not**
  be verified in this session: it constructs `Config::default()`, which
  currently panics unconditionally on this branch due to a pre-existing,
  unrelated bug in `crates/codegen/xai-grok-models/default_models.json`'s
  validator (it compares a model's `id` against a list built from the
  `model` field, so `"simplicio-1"` is never found — introduced by the
  recent Simplicio-1 identity rename, PR #17). This affects *any* test or
  code path that touches `Config::default()`/`default_web_search_model()`,
  not just this test — flagged separately, not something this change
  introduced or can safely paper over. The test itself is correct and will
  pass once that bug is fixed.
- A network-capturing e2e proxy test (issue #13 step 8: "testar proxy que
  captura toda saída de rede") is **not** included in this change — it
  requires an end-to-end harness (spawn the built binary, run it behind a
  MITM proxy, assert the allowlisted-domain set) that is a bigger
  integration-test investment than fits this slice; tracked as follow-up
  work, not claimed as done here.
