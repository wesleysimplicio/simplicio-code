# Canonical session registry

`session_registry` is the Code-side, provider-neutral index for issue #204. It
projects external Agent/Loop/Runtime sessions into spaces, projects and
workspaces; it is not a session runtime.

## Authority boundary

- The invoking external LLM and the existing Agent/Loop/Runtime remain the only
  authorities for conversation content, coordination and effects.
- `attach` returns an already-issued opaque `external_handle_id`. It has no
  process, filesystem, network, provider, model, Runtime or scheduler API, so
  attaching TUI, headless, ACP and workspace clients cannot spawn a duplicate.
- Persistence adapters may serialize `RegistrySnapshot` only. Prompt text,
  code, raw payloads, secrets and absolute paths are rejected or absent.
- Notifications are passive projections (`steals_focus == false`) and are
  deduplicated by session and notification kind.

## Lifecycle and reconnect

The lifecycle supports create, attach, detach, pause, resume, cancel and close.
Invalid transitions fail without advancing activity or cursors; cancel/close
and repeated attaches are idempotent. Cursor replay is limited to 1,024 events.
A validated new `host_instance_id` resets the cursor to zero and reports an
explicit `host_restart` resync; mismatched, delayed, stale, future and oversized
responses degrade with a fixed reason catalog while preserving the last cursor.

Snapshots are emitted as `simplicio.session-registry/v1` and restore v1 or the
N-1 `v0` schema. New optional resync/degraded fields default safely when reading
N-1 records. All IDs and display labels are bounded and validated before entry.

## Verification

Run locally without a provider or local LLM:

```bash
cargo test -p simplicio-runtime-client session_registry --lib
cargo bench -p simplicio-runtime-client --bench session_registry -- \
  --warm-up-time 0.1 --measurement-time 0.2 --sample-size 10
rustfmt --edition 2024 --check \
  crates/codegen/simplicio-runtime-client/src/session_registry.rs \
  crates/codegen/simplicio-runtime-client/benches/session_registry.rs
```

The benchmark measures cold create, warm list/detail, attach/detach, replay and
snapshot size/serialization after 100-session churn. Real multi-process
AgentHost/Loop Hub/Runtime E2E remains an external evidence lane: this module
does not start those services, and fixture/unit evidence must not be reported as
final E2E approval.
