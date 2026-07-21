# Mandatory coverage gate

The `coverage` job in `.github/workflows/ci.yml` measures line coverage for
the Simplicio-owned Rust boundary crates:

- `simplicio-runtime-client`;
- `simplicio-agent-client`;
- `xai-grok-models`.

The blocking threshold is **85% line coverage**. The job publishes the LCOV
file as a workflow artifact and fails when the threshold is not met. It is a
dependency of the `mandatory-gate` aggregate job, which is the single check to
protect in branch rules; individual matrix jobs remain useful diagnostics.

Coverage is deliberately scoped to owned boundary crates. It is not a claim
about the inherited `xai-grok-*` workspace. Missing or unobservable coverage
fails the job; it is never converted to zero or treated as a pass.
