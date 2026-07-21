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
and local promotion primitives. Release-event automation, external artifact
publication, and the shared installer remain dependencies of the ecosystem and
must supply real pins/digests before a production bundle can be promoted.
