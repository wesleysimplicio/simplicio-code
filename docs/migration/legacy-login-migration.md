# Design: legacy login/entitlement migration (issue #10)

> **Status: design only, pending #3/#4.** This document is a design pending
> the Simplicio login/entitlement system (#3) and the private gateway (#4).
> Neither exists in this repository yet — the auth path in this tree is
> still the direct-provider path described in
> [README.md#estado-do-produto](../../README.md#estado-do-produto)
> (`OPENROUTER_API_KEY` from the environment; `simplicio-code login`
> resolves to the legacy `auth.json`/OAuth2/device-code flow documented
> below, not a Simplicio-issued token). Building migration logic against a
> gateway contract that doesn't exist yet would be fictional, so this
> document catalogs the **real, current** config/session formats (step 1 of
> the issue: "catalogar formatos e versões de config e sessão") and proposes
> a manifest format, idempotency/resumability approach, and rollback
> strategy that a future implementation can follow once #3/#4 land. Nothing
> in this document should be read as already implemented.

## 1. Current config/session format inventory (verified against this tree)

This is the "what exists today" catalog the migration must read and
preserve. All paths are rooted at `grok_home()` — `$GROK_HOME` if set, else
`~/.grok` (`crates/codegen/xai-grok-config/src/paths.rs:28-47`).

| Artifact | Path | Format | Notes |
|---|---|---|---|
| Main config | `~/.grok/config.toml` | TOML | Loaded via `crates/codegen/xai-grok-config/src/loader.rs:15,84`; deserializes into `Config` (`crates/codegen/xai-grok-shell/src/agent/config.rs:1265`). No `schema_version`/`config_version` field exists today — see §1.2. |
| Auth/credentials | `~/.grok/auth.json` | JSON | `crates/codegen/xai-grok-shell/src/auth/manager.rs:287,300`; read/write in `crates/codegen/xai-grok-shell/src/auth/storage.rs` (`read_auth_json` L50, `write_auth_json` L193, atomic write + `auth.json.lock` advisory lock). Corrupt files are moved to `auth.json.corrupt.<millis>` (storage.rs:94) rather than deleted. |
| Auth token shape | (inside `auth.json`) | JSON | `GrokAuth` struct, `crates/codegen/xai-grok-shell/src/auth/model.rs:39` — `key`, `auth_mode` (`AuthMode`: `WebLogin` *deprecated*, `Oidc`, `External`, `ApiKey`), `refresh_token: Option<String>`, `expires_at`, `oidc_issuer`, `oidc_client_id`, org/team identity fields. |
| Sessions | `~/.grok/sessions/<encoded-cwd>/` | mixed | Per-project directory, `crates/codegen/xai-grok-config/src/paths.rs:159-170`. |
| Transcript | `.../chat_history.jsonl` | JSON Lines | Atomic rewrite, `crates/codegen/xai-grok-shell/src/session/acp_session.rs:1254-1293`. |
| ACP notification log | `.../updates.jsonl` | JSON Lines | Source of truth per `session/export.rs:3`. |
| Prompt history | `.../prompt_history.jsonl` | JSON Lines | `session/acp_types.rs:602`. |
| Feedback log | `.../feedback.jsonl` | JSON Lines | `session/commands.rs:594`. |
| Per-turn trace | `.../{session_id}/turn_N/streaming_partial.json` | JSON | `session/acp_session.rs:973`. |

### 1.1 Config top-level sections (from the `Config` struct)

`features`, `goal`, `doom_loop_recovery`, `auto_mode`, `config_models`,
`grok_com_config`/`auth` (provider auth config — **distinct** from the
device-login `auth.json` above), `shortcuts`, `hints`, `ui`, `toolset`,
`endpoints`, `telemetry`, `session`, `agent`, `repo_changes_dedup`, `skills`,
`compat`, `plugins`, `feedback`, `paths`, `cli`, `models`, `harness`,
`relay`, `remote`, `hub`, `worktree_pool`, `sandbox`, `mcp_servers`,
`disabled_mcp_servers`, `disabled_mcp_tools`, `subagents`, `memory`,
`compaction`, `managed_mcps`, `desktop` (opaque), `announcements`, `tips`,
`permission`, `tools`, `storage`, `suggestions`, `marketplace`,
`diagnostics`. A migration must be able to round-trip every section it
doesn't explicitly rewrite — see §3 (data-loss guard).

### 1.2 No existing version field — greenfield migration framework

There is **no** `schema_version`/`config_version` field in `config.toml` or
`auth.json` today, and no general config-migration framework. The only
precedent is ad hoc, field-level serde handling:

- `AuthMode::WebLogin` is marked deprecated but kept for deserializing old
  `auth.json` files, with `#[serde(alias = "grok")]` (model.rs, ~L22).
- `GrokAuth.has_grok_code_access` is similarly deprecated-but-kept.
- A `LEGACY_SCOPE` constant (model.rs:11) is a fallback scope key for "old
  devbox auth files."
- A **devbox-specific** auto-migration path exists
  (`crates/codegen/xai-grok-shell/src/auth/flow.rs:582-606`), currently
  short-circuited by a stub (`devbox_login_stub.rs:6`) — this is the closest
  existing analogue to "migrate an old auth shape on first run," and the
  manifest design below generalizes its shape rather than inventing an
  unrelated one.
- The auto-updater already tracks "N-1" *installer binaries* for rollback
  (`crates/codegen/xai-grok-update/src/auto_update.rs:2892,2929`) — this is
  a useful **precedent for the rollback mechanics** (§4) but is unrelated to
  config schema versions; it does not currently apply to config/auth files.

**Conclusion:** the manifest format and idempotent-migration runner proposed
below have nothing to extend — they must be designed and built once #3/#4
land, with `config.toml` gaining its first-ever `schema_version` field as
part of that work.

## 2. Migration manifest format (proposed)

A single JSON file, `~/.grok/migration/manifest-<from>-to-<to>.json`,
written *before* any mutation begins and updated after each completed step
(so it also serves as the resume checkpoint — see §3):

```json
{
  "manifest_version": 1,
  "from_client_version": "0.3.0-beta.1",
  "to_client_version": "1.0.0",
  "started_at": "2026-07-17T12:00:00Z",
  "steps": [
    { "id": "backup_config", "status": "done", "completed_at": "..." },
    { "id": "backup_auth", "status": "done", "completed_at": "..." },
    { "id": "device_login", "status": "pending" },
    { "id": "validate_entitlement", "status": "pending" },
    { "id": "rewrite_config_provider_refs", "status": "pending" },
    { "id": "remove_legacy_provider_config", "status": "pending" },
    { "id": "finalize", "status": "pending" }
  ],
  "backup_dir": "~/.grok/migration/backup-<timestamp>/",
  "rollback_available": true
}
```

Design properties (mapping directly to the issue's acceptance criteria):

- **Idempotent**: each step is a no-op if `status == "done"`; the runner
  always resumes from the first `pending`/`failed` step instead of
  restarting from scratch — satisfies "migração roda uma vez e pode ser
  retomada após interrupção."
- **Ordered, not parallel**: steps run strictly in the listed order so a
  partial run always leaves the system in one of a small, enumerable set of
  states (never "half-rewritten config with an in-flight login").
  `remove_legacy_provider_config` is deliberately **last**, after
  `validate_entitlement` succeeds — satisfies "falha de login mantém dados"
  (a failed `device_login`/`validate_entitlement` step never reaches the
  step that would remove the old provider config).
- **Every supported artifact from §1 is backed up** (`backup_config`,
  `backup_auth`) before any rewrite — satisfies "nenhum projeto, histórico
  ou configuração suportada é perdido." Session files (`chat_history.jsonl`,
  `updates.jsonl`, etc.) are never rewritten by this migration at all; only
  `config.toml` and `auth.json` are in scope, so they need no backup/restore
  path beyond the general filesystem backup for defense in depth.
- **Model alias never exposed**: `rewrite_config_provider_refs` only ever
  writes the existing `Simplicio-1` alias (already the display name per
  README) into config — it does not introduce a new user-visible slug, so
  "usuário não vê OpenRouter nem modelo interno" holds by construction, not
  by redaction after the fact.

## 3. Idempotency and resumability

The manifest file's existence *is* the resume signal: on startup, if
`~/.grok/migration/manifest-*.json` exists with `rollback_available: true`
and no step marked `failed-terminal`, the client resumes that manifest
instead of starting a new one. This directly targets the required test
matrix ("interrupção em cada etapa e retomada," "upgrade, downgrade e
upgrade repetido"):

- Interrupting between any two steps and relaunching must re-enter the
  runner at the first non-`done` step — this requires every step to be
  written to disk (`status: "done"`) only *after* its effect is durable
  (e.g. `auth.json` write completes and syncs before `backup_auth` is
  marked done), not before.
- Re-running an already-completed migration (upgrade run twice, or a
  downgrade followed by a repeated upgrade) must detect
  `to_client_version` already matches the installed version and short-circuit
  to `finalize` without re-running `device_login` — avoids repeated login
  prompts on a no-op upgrade.
- `validate_entitlement` failing (session expired, no entitlement, device
  offline) must leave `device_login`'s prior success untouched and mark only
  `validate_entitlement` as `failed` (not `failed-terminal`), so a retry
  after connectivity returns resumes correctly.

## 4. Rollback strategy

- `backup_dir` in the manifest holds byte-for-byte copies of every artifact
  a step is about to modify, taken immediately before that step's first
  write (not batched at start) — this bounds the backup to what was actually
  touched, which matters if the migration is aborted after only `backup_config`
  ran.
- Rollback restores from `backup_dir` and deletes the manifest, but
  **never restores `auth.json`'s legacy secrets once
  `remove_legacy_provider_config` has completed successfully and the new
  login has been confirmed working** — satisfies "rollback é seguro ... e
  não restaura segredos obsoletos" once entitlement is confirmed live.
  Rollback *before* that point is a straightforward restore, since no
  secret has been removed yet.
- Rollback is user-invokable (`simplicio-code migration rollback`, not yet
  implemented) and is itself idempotent: running it twice after a successful
  rollback is a no-op (manifest and backup dir are already gone).
- Rollback is bounded to the config/auth artifacts in §1; it never touches
  session/transcript files, which this migration never modifies.

## 5. Version support (N-2 / N-1 / beta atual)

The manifest's `from_client_version` is read from the currently-installed
binary's version string (see the existing `"0.3.0-beta.1"` convention in
`Cargo.toml`, e.g. `crates/codegen/xai-grok-shell/Cargo.toml`). Because
`config.toml`/`auth.json` have never carried a schema version, the
practical N-2/N-1/beta-atual support boundary is: any config/auth file this
migration's `backup_*` step can successfully **parse** with the current
`Config`/`GrokAuth` deserializers (which already tolerate several
deprecated fields via serde aliases, per §1.2) is in scope. A file that
fails to parse at all is left completely untouched and reported to the user
as "could not migrate automatically" rather than partially migrated —
never a silent data loss.

## 6. What this document deliberately does not do

- It does not implement `Command::Migration`/any manifest runner code —
  that depends on the login/entitlement contract from #3 and the gateway
  contract from #4, neither of which exist in this tree yet (confirmed:
  `simplicio-code login` today resolves to the pre-existing OAuth2/device
  auth flow in `crates/codegen/xai-grok-shell/src/auth`, not a
  Simplicio-issued token).
- It does not invent gateway request/response shapes. Once #3/#4 land, the
  `device_login`/`validate_entitlement` steps above should be filled in
  against their real contracts, not the placeholder description here.
