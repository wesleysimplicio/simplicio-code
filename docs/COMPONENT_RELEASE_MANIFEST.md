# Component release manifest

`component-release/v1` is the provenance boundary for the release train in
issue #57. `scripts/validate_component_release.py` requires one pinned entry
for Code, Runtime, Loop Hub, and Agent contracts, each with version, commit,
protocol, and a SHA-256 artifact digest. Floating `latest`, `main`, and `dev`
references are rejected.

The manifest digest is deterministic and can be attached to a bundle receipt.
Promotion, regeneration, and `code doctor/versions --json` can consume this
contract without downloading an unverified artifact. The validator does not
claim that release-event automation or a shared installer is already present.
