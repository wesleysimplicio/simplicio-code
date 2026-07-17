# Mapper incremental context (issue #6)

Issue #6 asks for the Mapper (the background repo scan started when a
folder is opened) to become a versioned, cached, incrementally-updated
source of context for the agent, with observable state, cancel/reindex,
and invalidation on branch/worktree/schema change.

## What existed before this change

Nothing in this Rust tree owned the map result itself. The only in-tree
code was `start_workspace_map()` in
`crates/codegen/simplicio-runtime-client/src/lib.rs` (called from
`crates/codegen/xai-grok-tools/src/computer/local/simplicio_runtime.rs:25`
on tool bootstrap), which:

- dedups by canonicalized workspace path in a process-global `HashSet`,
- spawns `simplicio runtime map --json --repo <path>` with
  `Stdio::null()` on every stream,
- never reads the child's stdout, never persists anything, and has no
  result type at all.

The actual mapping work happens entirely inside a separate, globally
installed `simplicio` Runtime binary (outside this Cargo workspace), which
writes its own output to `.simplicio/cache/resource-map-full.json` using
its own schema (`simplicio.runtime-resource-map/v1`) and its own versioning
(observed at `runtime.version = "3.5.2"` in this environment). There was no
contract in the Rust source for what that file contains, no cache the
in-tree code could read back, and no state machine.

## What this change adds

A new module, `crates/codegen/simplicio-runtime-client/src/map_cache.rs`
(exported from the crate root as `map_cache::{MapResult, MapState,
MapCache, budgeted_summary, compute_repo_hash, MAP_RESULT_SCHEMA_V1}`),
implementing the parts of the acceptance criteria that are genuinely
completable as a vertical slice without either vendoring or reimplementing
the external Runtime's actual repo-walking logic:

1. **Versioned contract** — `MapResult` is a `serde`-round-trippable struct
   tagged with `schema: "simplicio.map-result/v1"`
   (`MAP_RESULT_SCHEMA_V1`). Any cache entry not carrying that exact tag —
   wrong schema string, or a value with a mismatched `repo_hash`/
   `runtime_version` relative to its own cache key — is treated as a miss,
   never served. This is a genuinely versioned contract in the sense the
   issue asks for ("Versionar o contrato `simplicio.map-result`"), distinct
   from the pre-existing `simplicio.read-result/v1` file-read contract.

2. **Cache keyed by repo hash + Runtime version** — `MapCache` persists
   `MapResult` values to disk (one JSON file per `(repo_hash,
   runtime_version)` pair) and holds them in memory. `compute_repo_hash()`
   derives a `blake3` digest from the git `HEAD` ref, the resolved branch
   name, and the canonicalized worktree root — resolving the `.git` file
   indirection used by worktrees, so **a worktree checked out from the same
   repo gets its own hash** the moment its `HEAD` differs. Switching
   branches, switching worktrees, or upgrading the Runtime binary
   (different `runtime_version`) all naturally miss the old cache entry
   without any extra invalidation bookkeeping. `invalidate_repo()` also
   exists for the case where a caller wants to proactively evict every
   Runtime-version entry for a given repo (e.g. a forced re-map), and
   removes both the in-memory and on-disk copies.

3. **Observable states** — `MapState` is exactly the five states the issue
   asks for: `Waiting`, `Mapping`, `Ready`, `Degraded`, `Failed`.
   `MapResult::is_usable()` returns `true` only for `Ready`/`Degraded`,
   encoding the acceptance criterion "falha do mapa não permite leitura
   direta fora do Runtime" at the type level — a caller that only ever
   injects context when `is_usable()` is true can never surface a failed
   map as if it were real data.

4. **Fixed context budget** — `budgeted_summary(summary, budget_chars)`
   truncates a structural summary to at most `budget_chars` Unicode scalar
   values (never splitting a multi-byte character), appending a visible
   truncation marker so downstream telemetry/logging can tell a summary was
   cut rather than silently assuming it's complete. Handles the edge cases
   a naive `&s[..n]` byte-slice would panic or corrupt on (multi-byte UTF-8,
   budgets smaller than the marker itself, zero budget).

5. **No sensitive paths in the cache key** — `compute_repo_hash()` returns
   a `blake3` hex digest; the raw workspace path is hashed, never stored or
   returned, satisfying "estado/duração visíveis sem vazar caminhos
   sensíveis em telemetria" for the cache-key portion of that requirement.

Tests (`crates/codegen/simplicio-runtime-client/src/map_cache.rs`, `#[cfg(test)]
mod tests`, 22 tests total in the crate, 19 new): budget truncation
(under-budget passthrough, over-budget truncation, tiny/zero budgets,
multi-byte boundaries), schema tagging and round-trip serialization, state
usability rules, cache put/get, cross-process disk recovery, cold-cache
miss handling, stale-schema rejection, write dedup (identical value ⇒ no
disk write, verified via unchanged mtime), overwrite-on-real-change,
independent entries per Runtime version, `invalidate_repo` dropping every
version for a repo hash while leaving other repos untouched (both in
memory and on disk), repo-hash stability/change across branch switches and
worktree `gitdir:` indirection, and a check that the hash never contains
the raw filesystem path as a substring.

```
$ cargo test -p simplicio-runtime-client
running 23 tests
test result: ok. 22 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out
```

(The 1 ignored test, `reads_a_real_file_through_runtime_mcp`, predates this
change and requires an installed Simplicio Runtime binary — unrelated to
this work.)

`cargo clippy -p simplicio-runtime-client --all-targets` is clean except two
pre-existing `disallowed_methods` warnings in `lib.rs` (bare
`Path::canonicalize` instead of `dunce::canonicalize`) that predate this
change and were not introduced by it; the new `map_cache.rs` module uses
`dunce::canonicalize` throughout.

## What is deliberately NOT in this change (open acceptance criteria)

- **Injecting the summary into the Simplicio-1 prompt context.** This
  module gives the agent-facing code a place to *get* a budgeted summary
  from, but nothing in `crates/codegen/xai-grok-agent`'s prompt assembly
  calls into it yet. Wiring `budgeted_summary()` into
  `crates/codegen/xai-grok-agent/src/prompt/context.rs` is the natural next
  step but touches prompt-context tests this session could not compile
  (see `docs/brand-audit.md`'s "Known blocker" — the whole `codegen`
  workspace needs `protoc`, which this sandbox doesn't have), so it was
  left as a follow-up rather than risk an unverified change to
  request-shaping code.
- **Watcher-driven incremental updates.** "Atualizar apenas arquivos
  alterados por watcher" needs an actual filesystem watcher wired to the
  external Runtime's incremental re-map path (or a re-implementation of the
  Runtime's own indexer in-tree) — out of scope for a single session; this
  slice only versions/caches *whatever* result is handed to it, it does not
  produce partial/incremental results itself.
- **TUI/headless cancel, reindex, diagnostics surfaces.** No UI wiring
  exists yet for cancelling an in-flight map, forcing a reindex, or
  inspecting cache state from the TUI or `--headless` mode. `MapCache`
  exposes `len()`/`is_empty()` for a future diagnostics command to report
  cache size without leaking paths, and `invalidate_repo()` for a future
  "reindex" command, but no command currently calls them.
- **Concurrency between two sessions.** Not tested. `MapCache` itself does
  no file locking; two processes writing the same `(repo_hash,
  runtime_version)` key concurrently could race (last write wins, no
  corruption since each write is a single `fs::write` of a complete JSON
  document, but no explicit lock file the way `.simplicio/index.lock`
  — observed in this working tree, presumably written by the external
  Runtime — implies the real Runtime already does for its own state).
- **Cold/warm/incremental benchmarks and the required 85% coordinator
  coverage.** No "coordinator" component exists yet to benchmark or cover
  at that threshold — this slice is the versioned contract + cache + budget
  primitives the coordinator would be built on top of, not the coordinator
  itself.
- **Integration/system tests** against a real small repo / monorepo /
  worktree / two-concurrent-sessions scenario, and a real `simplicio`
  Runtime binary. The unit tests above use synthetic `.git` directories
  under `std::env::temp_dir()`, not real checkouts.
