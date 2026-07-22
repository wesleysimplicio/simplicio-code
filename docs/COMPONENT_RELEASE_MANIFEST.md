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

Authenticated ecosystem producers deliver `simplicio.release-event/v1` through
the `simplicio-component-release-v1` repository-dispatch event. The 15-minute
workflow budget validates the canonical bundle digest and generated-client
digest, regenerates the client, and updates a single deduplicated PR branch.
`scripts/release/apply_component_release.py` is offline by design: it cannot
resolve or download `latest`. Protocol-range changes are recorded as
`migration_required` in the release receipt so promotion cannot silently mix
contracts. Re-running the same event with `--check` proves byte reproducibility.

The signed-event workflow additionally materializes the operator trust root and
four immutable HTTPS artifacts, verifies their digests and compatibility before
writing the manifest, replay state, and a `release-bump-receipt/v1` containing
the signing key, producer sequence, canonical bundle digest, and independently
recomputed artifact digests. It requires the Runtime pin to attest the digest of bindings
reproduced from the repository schema, and uses an event-id branch/PR with
durable replay protection.
Missing or revoked keys, malformed events, stale sequences, incompatible
protocols, and incorrect artifacts fail closed; the workflow only prepares a
Code bump and never becomes a Runtime/map/queue authority.

The platform-neutral promotion harness consumes those already downloaded,
verified inputs without network discovery. It copies all four pins to a private
inactive slot, recomputes their digests after copying, exposes only that slot to
the caller's canary through `SIMPLICIO_BUNDLE_SLOT`, and changes `active` only
after a successful canary:

```text
PYTHONPATH=. python3 scripts/release/promote_component_bundle.py promote \
  --manifest config/component-bundle.json --artifacts /path/to/artifacts \
  --slots /path/to/component-slots --canary-command /path/to/installed-e2e
PYTHONPATH=. python3 scripts/release/promote_component_bundle.py rollback \
  --slots /path/to/component-slots
```

The harness itself starts no component authority. A failed digest, concurrent
promotion, or failed canary removes the inactive slot and preserves `active`;
rollback swaps the complete `active` and `previous` directories. Real publisher
endpoints and clean installed Windows/Linux/macOS executables are intentionally
not inferred from repository fixtures and remain external evidence blockers for
issues #57 and #110.
