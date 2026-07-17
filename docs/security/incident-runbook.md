# Secret-leak incident runbook

Scope: what to do when a real credential (OpenRouter, OpenAI, xAI, GitHub,
AWS, Slack, or any other provider/vendor secret used by Simplicio Code) is
suspected or confirmed to have been exposed — committed to git, printed to a
log/telemetry sink, included in a crash report, or shipped in a release
artifact.

This complements [`SECURITY.md`](../../SECURITY.md) (which covers *reporting*
a vulnerability to us) with the *internal* response process for a leak once
it's known. It does not replace HackerOne intake for external reports.

## 1. Detection

A leak can surface through any of these paths — treat a hit from any of them
as real until proven otherwise:

- **CI secret-scan gate** (`.github/workflows/ci.yml`, `secret-scan` job,
  gitleaks against the PR diff or push). A red check here blocks merge; do
  not bypass it to land the PR — see §2.
- **Local pre-push/manual scan**: `gitleaks detect --source . -c
  .gitleaks.toml` run against the working tree or full history.
- **Manual report** via HackerOne (`SECURITY.md`) or an internal report.
- **Log/telemetry review**: a raw `Authorization` header, API key, or bearer
  token observed in Sentry, OTel export, Mixpanel, or local debug logs. See
  `crates/codegen/xai-grok-secrets/src/sanitizer.rs` for the redaction pass
  that is supposed to catch these before export — a leak here means either a
  new secret shape isn't covered by `MATCH_ANY`/`redact_secrets`, or a log
  call site bypasses the sanitizer entirely (as in the fix in this PR for
  `crates/codegen/xai-grok-sampler/src/client.rs`, where a raw `api_key` was
  passed straight to `tracing::debug!` on an error path instead of through
  the redaction layer or as shape-only metadata).
- **Crash-dump / binary inspection**: a secret embedded in a build artifact
  or a native crash report (`crates/codegen/xai-crash-handler`).

## 2. Immediate response (first 30 minutes)

1. **Do not push a "quick fix" that only removes the file from HEAD.** Git
   history retains the blob; a force-push/rewrite is a separate, coordinated
   step (see §5) and must not be done unilaterally on a shared branch.
2. **Identify the secret's owner/scope**: which provider (OpenRouter, OpenAI,
   xAI, GitHub, AWS, Slack, ...), which environment (dev/staging/prod/shared
   CI secret), and its blast radius (client-distributed vs. server-only).
3. **Revoke/rotate at the source** — this is an organizational action outside
   what this repository's code can perform or verify:
   - Rotate the credential at the provider's dashboard/API (OpenRouter key
     rotation, GitHub PAT revocation, AWS IAM key deactivation, etc.).
   - Update the gateway/secret-manager entry consuming it (if/when a
     centralized secret-manager service exists for this deployment; note:
     as of this runbook, no such gateway service is present in this
     repository — secrets are currently supplied via env/config at the
     client/CLI layer, which is exactly what issue #9 is tracking down).
   - Confirm no other environment (staging, a teammate's local `.env`, a
     forked CI config) still references the now-revoked value.
4. **Contain**: if the leak is in a *public* release artifact or public repo
   history, treat the credential as compromised immediately — rotation is
   not optional and is not gated on completing the rest of this runbook.

## 3. Scoping the exposure

- **Source/history**: `gitleaks detect --source . -c .gitleaks.toml
  --log-opts="--all"` for the full reachable history, or `gitleaks detect
  --source . -c .gitleaks.toml --log-opts="<since-commit>..HEAD"` to scope a
  specific range.
- **Release artifacts**: check whether the affected commit range shipped in
  a tagged release (`git tag --contains <bad-commit>`), and whether the
  secret could reach a distributed binary (env var baked in at build time,
  vendored config, etc.) — grep the built binary's strings for the value
  (see §6, "canary secrets" testing, for how to check this class of leak).
- **Logs/telemetry/crash reports**: search Sentry/OTel/Mixpanel backends (or
  local log files) for the literal secret value or its known prefix
  (`sk-`, `xai-`, `ghp_`, `AKIA`, etc.) across the exposure window. Anyone
  running this search must handle the raw value with the same care as the
  secret itself (short-lived access, no copy-paste into tickets/chat).

## 4. Communication

- Open an internal incident record referencing this runbook and the
  detection source (CI run URL, HackerOne report ID, log query, etc.).
- Notify: the credential's owning team, whoever owns the provider account
  (billing/security contact), and — if client-distributed or externally
  reachable — follow the disclosure process in `SECURITY.md` rather than a
  public GitHub issue.
- Do not include the raw secret value in any ticket, chat message, commit
  message, or this runbook. Reference it by provider + last-4/prefix only
  (e.g. "OpenRouter key ending `...ab12`").

## 5. Recovery

1. Confirm rotation is complete and the old credential is confirmed
   rejected by the provider (test a call with the old key and expect 401).
2. Land the code fix (redaction gap closed, log call site fixed, etc.) and
   confirm `cargo test -p xai-grok-secrets` and the CI `secret-scan` job are
   green on the fix.
3. If the secret is reachable in git history, decide with the repo owner
   whether history rewrite (`git filter-repo` / BFG) is warranted — this is
   disruptive to every clone/fork and is a judgment call weighed against
   "the credential is already rotated and useless," not a default action.
4. Add a regression test: a redaction unit test with a canary (fake) value
   in the same shape as the leaked secret, asserting it's caught by
   `redact_secrets` / `MATCH_ANY`, and/or a `gitleaks` rule/allowlist review
   if the shape wasn't covered by the default ruleset.
5. Close the incident record with: what leaked, how it was detected, when it
   was rotated, and the regression test/CI change that prevents recurrence.

## 6. Required tests going forward (issue #9 acceptance criteria)

- **Secret scan, full history + diff**: `secret-scan` CI job
  (`.github/workflows/ci.yml`), gitleaks with `.gitleaks.toml`.
- **Redaction unit tests**: `cargo test -p xai-grok-secrets` — covers vendor
  key shapes, PEM blocks, bearer/JWT, URL query-param scrubbing, and
  user-path anonymization. Extend this suite whenever a new provider/secret
  shape is introduced.
- **Canary secrets in logs/telemetry/crash paths**: see
  `invalid_api_key_error_path_never_logs_raw_secret` in
  `crates/codegen/xai-grok-sampler/src/client.rs` for the pattern — a
  canary (fake, uniquely-identifiable) secret string fed through the code
  path under test, with a capturing `tracing::Subscriber` asserting the raw
  value never appears in any emitted field.
- **Binary/package string inspection**: best-effort, not yet automated in
  CI. Locally: `strings target/release/<binary> | grep -E
  'sk-|xai-|AKIA|ghp_|BEGIN.*PRIVATE KEY'` (or an equivalent tool on
  platforms without `strings`) against a release build, expecting no
  matches. This is a known gap — tracked as follow-up, not fabricated as
  already automated.
- **Rotation-under-traffic test**: not implemented — there is no
  centralized secret-manager/gateway component in this repository today to
  exercise a live rotation against. Tracked as a gap pending that
  infrastructure existing.

## Known gaps (explicitly not covered by this runbook or the current CI gate)

- No centralized secret-manager/gateway service exists in this repo to
  rotate against programmatically; secrets are supplied via client-side
  env/config today. Centralizing them is out of scope for a single PR and
  is the larger arc of issue #9.
- Actual revocation/rotation of any real, currently-deployed credential is
  an operational action outside what a code change can perform or prove —
  it must be done by whoever holds provider account access.
- GitHub branch-protection settings (requiring the CI checks in
  `.github/workflows/ci.yml` before merge) require repository-admin access
  and are not something this PR can configure.
