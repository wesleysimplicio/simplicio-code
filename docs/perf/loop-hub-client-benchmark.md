# Loop Hub client hot-path benchmark

Issue #55 requires replay-safe idempotency without adding a Code-owned queue or
scheduler. The benchmark measures the only new per-submit hot path: encoding the
session/turn/goal causal identity into an unambiguous, length-prefixed key.

## 2026-07-22 baseline

Environment: Linux x86-64 Codex Cloud container, Rust toolchain from
`rust-toolchain.toml`, Criterion 0.6.0, release benchmark profile.

```text
cargo bench -p simplicio-runtime-client --bench loop_hub_idempotency -- \
  --warm-up-time 1 --measurement-time 2 --sample-size 20

loop_hub_idempotency_key  time: [143.55 ns 146.56 ns 149.30 ns]
```

The median estimate is **146.56 ns per submit key** (approximately 6.82 million
keys/second on this runner). This benchmark performs no I/O and spawns no
Runtime, Mapper, model worker, or scheduler. Re-run it when the causal key wire
format changes; results from different machines are not directly comparable.
