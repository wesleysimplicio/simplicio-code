# Code internal-state migration

This document records the Code-side implementation for issue #99. The accepted
ecosystem decision is [ADR 2026-07-21 — Binary internal formats and edge-only
JSON](https://github.com/wesleysimplicio/simplicio-runtime/blob/main/docs/ADR-2026-07-21-BINARY-INTERNAL-FORMATS.md).

## Ownership map

| Artifact | Producer | Consumer | Canonical format | Compatibility |
| --- | --- | --- | --- | --- |
| Runtime repository-map cache | Runtime map adapter / Code cache | mapper-context consumer | HBI adapter, `simplicio.map-result/v1` | Explicit JSON upgrade reader only; no normal JSON fallback |
| Managed configuration sync marker | managed-config sync | staleness and policy gates | HBI, `simplicio.managed-config-marker/v1` | Old JSON marker is not read as live state |
| Append-only migration/evidence records | Code migration and audit tools | release/evidence readers | HBP v1 | Hash-chain verification is fail-closed |
| Disk-preflight execution receipt | disk-budget wrapper | build/performance evidence reader | HBP v1, `code.record` | Atomic single-record publication; no JSON fallback |
| Human Code runtime configuration | operator | Runtime client | strict typed TOML | Unknown keys and unsupported schema versions fail |
| Runtime MCP / provider JSON | external Runtime/provider | boundary adapter | external protocol only | Raw JSON terminates at the adapter |

`crates/codegen/simplicio-code-formats` contains the Runtime-compatible HBP/HBI
containers, strict TOML model and atomic migration primitive. HBI validation
checks magic, version, endianness, alignment, header/total length, schema
fingerprint, section bounds, overlap, zero padding, SHA-256 section checksums
and the aggregate integrity stream before exposing a section slice. HBP uses
Runtime's `HBP1` header, length-prefixed UTF-8 rows, genesis-linked SHA-256
hashes and explicit topic/provenance ownership.

## Legacy migration contract

Legacy conversion is explicit and one-way. A caller must request dry-run or
commit, the parser is bounded, the source is copied to a `.legacy.bak` backup,
and the target is published through a same-directory synced temp file and
rename. A failed or truncated conversion leaves the legacy source untouched.
`MapCache::load` only reads HBI; it never silently falls back to JSON.

The legacy reader in `MapCache::migrate_legacy` is scheduled for removal after
2026-12-31. It is classified in `config/json-boundaries.toml` as an exact,
owned migration boundary.

## Runtime dependency

Runtime publishes HBI v1 and HBP v1 through the binary-contract receipt
(`simplicio_binary_contracts`). Code's HBI encoder reproduces the Runtime
golden-vector digest `b2599e6064597fe74fac2f73118cc9b278f2d6623d349491ea3fbab582a5656d`
and its HBP adapter emits the Runtime `HBP1` row layout. The receipt remains
the release-time authority for semantic versions and specification digests;
the Code tests do not start an LLM, provider, scheduler or external service.

## Measurement

Only observations are recorded here; unavailable values remain explicit rather
than estimated.

| Workload | Before bytes | After bytes | Before load | After load | RSS / allocations |
| --- | ---: | ---: | ---: | ---: | --- |
| MapCache representative result | `null` — no frozen baseline artifact | `null` — no frozen baseline artifact | `null` — no profiler captured | `null` — no profiler captured | `null` — no profiler available |
| Managed marker | `null` — no captured baseline artifact | `null` — no captured migrated artifact | `null` — no profiler available | `null` — no profiler available | `null` — no profiler available |

The Code-owned codec hot paths can be measured without Runtime or network access:

```sh
cargo run --release -p simplicio-code-formats --example format_benchmark -- 10000
```

The benchmark prints iteration count, actual encoded size, and measured mean
microseconds per operation. Peak RSS remains an external observation and must
be captured with the platform tool (for example `/usr/bin/time -v` on Linux),
not inferred by the benchmark.

Observed on 2026-07-23 in the native Runtime MCP test container (Rust release
profile, 1,000 iterations; the runner does not expose a stable CPU model):

| Operation | Iterations | Artifact bytes | Mean µs/op | Peak RSS |
| --- | ---: | ---: | ---: | --- |
| HBI warm validate/read, 64 KiB payload | 1,000 | 65,704 | 381.788 | `null` — `/usr/bin/time` is unavailable in the container |
| HBP decode, 32 records | 1,000 | 14,351 | 63.731 | `null` — `/usr/bin/time` is unavailable in the container |

`cargo llvm-cov -p simplicio-code-formats --all-targets --summary-only`
measured 85.30% line coverage and 88.27% region coverage. This toolchain did
not expose branch coverage (`-`), so no branch percentage is inferred.

The local Python scanner and package smoke tests are reproducible offline. The
Runtime MCP binary-contract receipt and the Rust HBI/HBP codec tests are the
external compatibility gates; they are provider-free and do not require a
local LLM.

## Release boundary gate

The source scanner's strict mode intentionally continues to report the exact
`migration_pending` entries above; enabling the strict source gate does not
mean the HBP/HBI migration is complete. Release packaging has a narrower,
release-blocking check: after the manifest and SBOM are generated,
`scripts/check_package_contents.py` scans every JSON-family file in `dist/`.
Only exact `[[package_output]]` entries in `config/json-boundaries.toml` may
pass, and each exception must have an owner, producer, consumer, lifecycle,
reason, target format and unexpired date. A newly emitted JSON artifact fails
the package job unless it is reviewed and added as its own exact exception.
Repository and package exception paths must also be canonical, relative paths:
absolute paths, parent traversal, globs and platform-specific separators are
rejected. Pinned strict-lane scope entries must continue to name existing files,
so deleting or renaming an audited producer cannot silently turn the lane green.
