# Component release manifest

`component-release/v1` is the provenance boundary for the release train in
issue #57. The normative shape is
[`docs/contracts/component-release-v1.schema.json`](contracts/component-release-v1.schema.json).
It requires one pinned entry for Code, Runtime, Loop Hub, and Agent contracts,
each with version, commit, protocol, and a SHA-256 artifact digest. Floating
`latest`, `main`, and `dev` references are rejected.

The manifest digest is deterministic and can be attached to a bundle receipt.
The generated Runtime client fingerprint is produced with:

```text
python3 scripts/release/generate_component_client.py \
  --schema docs/contracts/component-release-v1.schema.json \
  --out crates/codegen/simplicio-runtime-client/src/generated.rs
```

`RuntimeClient::spawn_in_with_manifest` performs the compatibility handshake:
the installed Runtime must announce the exact pinned version, commit, artifact
digest, and supported protocol range. Missing provenance is a hard failure.

`BundleStore` stages into a digest-named inactive slot, runs a caller-supplied
canary, then swaps `active`/`previous` using filesystem renames. A rejected
canary does not change the active bundle, and rollback never touches the
session/config directory. The update lock prevents two promoters from
starting the update at once; the store does not start Runtime or map/queue
authorities.

This repository provides the contract, deterministic generation, handshake,
and local promotion primitives. The Code-side release-event boundary is
[`docs/contracts/release-event-v1.schema.json`](contracts/release-event-v1.schema.json).
`SignedReleaseEvent` verifies an Ed25519 signature over canonical payload bytes
using only caller-provided trusted keys, then checks the event id, producer
sequence, manifest compatibility, and manifest digest. `BundleStore::ingest_release_event`
persists event ids, rejects conflicting or stale events, checks active-receipt
drift, and invokes the existing stage/canary/active/previous promotion path.
Duplicate delivery is a no-op and never runs the canary again.

This is a Code-side ingestion boundary, not an external release publisher. It
does not fabricate events, fetch `latest`, publish artifacts, or claim installed
Windows/Linux/macOS E2E. External ecosystem event delivery, signed provenance
publication, generated bump PR automation, and installed cross-platform evidence
remain dependencies of issue #110.
