# Token / latency benchmark (issue #12)

Status: **partial**. This document covers what is measurable inside this repo
today, plus a deliberately non-statistical N=1 attempt against the Simplicio
Runtime CLI/MCP. The installed CLI was found, but MCP content was not usable in
this run, so native reads are explicitly marked
`UNVERIFIED| runtime capability gap`. It is not the full, CI-reproducible
"Runtime cold/warm/incremental vs direct read" comparison the issue asks for — see
[Scope and what's blocked](#scope-and-whats-blocked).

## Goal

Issue #12 asks for quantitative evidence that the Simplicio Runtime, the
Mapper, and "concise mode" reduce tokens without reducing correctness. That
requires comparing at least two real, comparable code paths under a versioned
corpus of tasks with verifiable expected output, and gating any perf win on
not regressing the success rate.

## What exists in this repo to benchmark

- `crates/codegen/xai-grok-tools/src/computer/local/simplicio_runtime.rs` —
  `SimplicioRuntimeFs`, the fail-closed filesystem the agent uses for reads.
  It shells out over MCP (stdio, JSON-RPC) to an external `simplicio` binary
  resolved via `SIMPLICIO_BIN` or `PATH`.
- `crates/codegen/simplicio-runtime-client` — the typed MCP client that talks
  to that binary (`RuntimeClient::spawn_in`, `read_file`, `start_workspace_map`).
- **No implementation of the Runtime/Mapper server itself lives in this
  repository.** `resolve_binary()` in `simplicio-runtime-client/src/lib.rs`
  errors with `RuntimeNotFound` if no `simplicio`/`simplicio.exe` is found on
  `SIMPLICIO_BIN`/`PATH`. There is no workspace member here that builds one —
  whether `runtime_attempt` below succeeds or fails depends entirely on what,
  if anything, happens to be installed on the machine running the benchmark.

So the two code paths this repo can drive *end to end* are:

1. **`direct_read`** — `std::fs::read`, the same primitive `LocalFs` (the
   write/fallback path) uses.
2. **`runtime_attempt`** — spawning the real MCP client exactly as
   `SimplicioRuntimeFs::read_file` does (fresh process + full
   `initialize`/`notifications/initialized` handshake + one `tools/call` per
   attempt — i.e. a cold path every time, since this harness does not reuse a
   warm client the way `SimplicioRuntimeFs` does across multiple reads),
   against whatever Runtime binary `resolve_binary()` finds. When it returns
   content, the harness now computes `approx_tokens`, byte length, and exact
   byte-for-byte equality with `direct_read`; otherwise it emits the required
   `UNVERIFIED| runtime capability gap` marker.

## Scope and what's blocked

The issue's step-by-step calls for comparing "leitura direta" against
"Runtime frio/quente/incremental" (cold/warm/incremental), plus Mapper-backed
context compression and "modo conciso" token accounting. This repository does
not implement any of that — it only ships the MCP *client*. Concretely:

- There is no `simplicio` binary target anywhere in this workspace
  (`grep -rn "runtime map\|serve --mcp" crates` only turns up the client
  invoking those subcommands, never a server implementing them).
- The [Results](#results) run below happened to find a real `simplicio.exe`
  (Runtime v3.5.2) already installed on the benchmarking machine's `PATH` —
  it is **not** built from this repository and is not something CI or a
  fresh checkout of this repo can rely on. Treat the runtime_attempt numbers
  below as "what one real Runtime binary did on one machine, once," not as a
  reproducible-from-this-repo baseline.
- This harness spawns a brand-new process and does the full MCP handshake on
  every single `runtime_attempt` call — there is no cold/warm/incremental
  distinction yet (that requires reusing one `RuntimeClient` across repeats
  the way `SimplicioRuntimeFs` does, plus a way to force/observe Mapper
  cache state, which is out of scope for this change).
- Mapper-driven context compression and "concise mode" token accounting are
  not exercised by `simplicio_file_read` at all in this run.

**The CI-reproducible version of this comparison is blocked on a
Runtime/Mapper implementation that ships with (or is built by) this repo**
(tracked as #5/#6 per the issue's own passo-a-passo). Once that lands,
`crates/codegen/simplicio-perf-bench` should be extended to (a) reuse one
warm client across repeats to get real cold-vs-warm numbers, and (b) assert
which binary answered `doctor`/`--version` so results are attributable to a
known, versioned Runtime build rather than "whatever happens to be on PATH."

## Methodology

### Corpus (versioned, no private code or user prompts)

Fixtures live in `crates/codegen/simplicio-perf-bench/fixtures/` and are
synthetic — no real repository content, no captured prompts:

| Task id                 | Fixture                                          | Purpose |
|--------------------------|--------------------------------------------------|---------|
| `read_small`             | `fixtures/small.rs` (~460 bytes)                 | latency floor |
| `read_medium`            | `fixtures/medium.rs` (~10.6KB, 300 lines)         | typical single-file read |
| `read_monorepo_nested`   | `fixtures/monorepo/pkg-c/nested/deep/module.rs`   | monorepo-style path resolution, 4 dirs deep |
| `read_large_synthetic`   | generated at bench time, 4MiB, deterministic xorshift64* content (not committed, to avoid bloating git) | large-file behavior |
| `read_invalid_path`      | a path that does not exist                        | "invalid map" / fail-closed path — must error cleanly, not partially succeed |

### Golden-task correctness gate

Every task has a documented expected outcome (`read_invalid_path` is
*expected* to fail; the other four are *expected* to succeed). The harness
reports `success_rate` per task/path so correctness is checked before any
token/latency number is trusted, per the issue's acceptance criteria. For a
valid Runtime read, success means that returned bytes match the direct fixture
byte-for-byte; a Runtime call that returns no content or mismatched content is
not counted as a correctness success.

### Metrics

- **Latency**: wall-clock per read, `Instant::now()`/`elapsed()`, over N
  repeats per task (`--repeats`, default 20). Reported as p50, p95, mean, and
  sample standard deviation in milliseconds.
- **Tokens**: `approx_tokens = byte_len / 4.0`. A documented,
  dependency-free **approximation**, not a real model tokenizer count — see
  `BYTES_PER_APPROX_TOKEN` in `crates/codegen/simplicio-perf-bench/src/lib.rs`.
  Treat it as a relative trend line, not an absolute token count for any
  specific model.
- **Success rate**: fraction of repeats that returned the expected outcome;
  valid Runtime reads must also match direct bytes exactly.
- **Content comparability**: `content_bytes`, `content_matches_direct`, and
  `approx_tokens` are populated from the Runtime response when MCP returns
  readable content. `null` means no comparable content was returned.

### Repeats / variance

The harness defaults to 20 repeats per task/path. **The run captured in
[Results](#results) below used `--repeats 1`**, not 20 — see the honesty note
there. p50/p95/stdev are still reported by the harness for any repeat count,
but with N=1 they degenerate to a single point estimate; there is no variance
data in this run.

## Running it

```sh
scripts/bench/run.sh                        # default: 20 repeats, prints JSON
scripts/bench/run.sh --repeats 50 --out /tmp/run.json
scripts/bench/run.sh --check-against crates/codegen/simplicio-perf-bench/baselines/run_2026-07-17.json
```

Or directly via cargo:

```sh
cargo run -p simplicio-perf-bench --bin simplicio-bench -- --repeats 30 --out report.json
cargo run -p simplicio-perf-bench --bin simplicio-bench-check -- --baseline base.json --current report.json
```

## Regression gate

`crates/codegen/simplicio-perf-bench` ships `find_regressions()` (unit-tested
in `src/lib.rs`, 7 passing tests) and the `simplicio-bench-check` binary:
given a committed baseline report and a fresh run, it flags any
task/path/metric where

- `latency_p50_ms` grows by more than `REGRESSION_THRESHOLD_PCT` (10%), or
- `approx_tokens` grows by more than 10%, or
- `success_rate` drops at all (any correctness regression is flagged
  regardless of percentage, matching "nenhum ganho aceito se reduzir taxa de
  sucesso").

`simplicio-bench-check` exits non-zero if any regression is found, so it can
gate CI once a stable baseline machine/profile is chosen. It is **not** wired
into CI in this change — that requires picking a dedicated, stable benchmark
runner (out of scope here) and, per the caveat above, a Runtime binary this
repo actually controls. Running it locally/manually is supported today.

## Results

Real, measured output, single sample per task/path
(`cargo run -q -p simplicio-perf-bench --bin simplicio-bench -- --repeats 1`,
debug build, Windows; Runtime CLI 3.5.2 on `PATH`). The committed JSON under
`baselines/` is historical evidence from the original harness and is not a
statistical baseline; this N=1 run must not be used as one.

**Honesty note on sample size**: this is N=1 per cell, not the 20+ repeats
the harness defaults to and that a trustworthy p50/p95/stdev needs. Treat
every number below as a single anecdotal data point, not a trend or baseline.
Each `runtime_attempt` pays a fresh process and MCP handshake. The installed
Runtime CLI was reachable, but the MCP capability exchange/read path returned
an invalid JSON response (JSON delimiter error at line 1 column 10181) in this
run; the known `tools/list` timeout after 30 seconds is tracked separately as
Runtime #3298.

| Task | Path | Expected | Correct | Latency (ms) | approx tokens | Content comparable |
|---|---|---:|---:|---:|---:|---|
| `read_small` (402B) | `direct_read` | success | 1/1 | 0.44 | 100.5 | baseline |
| `read_small` (402B) | `runtime_attempt` | success | 0/1 | 3190.61 | n/a | no; MCP invalid JSON |
| `read_medium` (8.7KB) | `direct_read` | success | 1/1 | 1.78 | 2217.5 | baseline |
| `read_medium` (8.7KB) | `runtime_attempt` | success | 0/1 | 3015.58 | n/a | no; MCP invalid JSON |
| `read_monorepo_nested` (353B) | `direct_read` | success | 1/1 | 1.74 | 88.25 | baseline |
| `read_monorepo_nested` (353B) | `runtime_attempt` | success | 0/1 | 3053.55 | n/a | no; MCP invalid JSON |
| `read_large_synthetic` (4MiB) | `direct_read` | success | 1/1 | 8.55 | 1,048,576 | baseline |
| `read_large_synthetic` (4MiB) | `runtime_attempt` | success | 0/1 | 2991.28 | n/a | no; MCP invalid JSON |
| `read_invalid_path` | `direct_read` | fail | 1/1 | 0.07 | n/a | n/a |
| `read_invalid_path` | `runtime_attempt` | fail | 0/1 | 2912.62 | n/a | n/a |

Key takeaways from this run:

- **Correctness**: `direct_read` matched all five expected outcomes (four
  successes and one fail-closed invalid path). The Runtime path returned no
  readable content in any task, so all five Runtime cells are
  `UNVERIFIED| runtime capability gap`; even the invalid path is unverified
  because MCP failed before the tool call. The 4MiB case now really addresses
  `.generated/large_synthetic.bin` inside the Runtime workspace; its failure is
  MCP capability failure, not an accidentally-invalid fixture path.
- **Latency**: direct-read values are local native fallback measurements and
  must not be compared as Runtime performance because MCP was unavailable.
  Runtime attempts took roughly 2.9–3.2 seconds while paying process spawn and
  MCP handshake cost, so they are cold-attempt observations only.
- **Tokens/content**: Runtime `approx_tokens`, `content_bytes`, and
  `content_matches_direct` are all `null` here. Therefore this run provides no
  evidence that the Runtime path returns content comparable for token counts;
  a successful MCP response must populate and compare those fields before any
  token claim is made.

## Update 2026-07-17: repeated sample (N=3) + warm-client-reuse variant

The original [Results](#results) run above was explicitly N=1 (a single
sample per task/path) and had no warm-client-reuse variant. This update adds
both, extending the same harness (`crates/codegen/simplicio-perf-bench`) —
no rewrite:

- **`--repeats 3`** (up from the N=1 run above). 3 repeats, not the harness
  default of 20: on this machine each cold `runtime_attempt` handshake
  attempt takes 14-30 seconds (see below), so 20 repeats x 5 tasks x 2 cold
  paths would take on the order of an hour. 3 repeats is still a real
  repeated sample (enough to compute a non-degenerate p50/p95/stdev) within a
  few minutes of wall-clock time. This remains a small sample; do not treat
  N=3 as statistically robust, only as "more real evidence than N=1."
- **`LatencyStats` (already in `src/lib.rs` from the original PR)** computes
  p50/p95/mean/sample-stdev from the actual per-repeat millisecond samples —
  this update didn't need to add that math, only to actually run with
  `repeats > 1` so the numbers are non-degenerate.
- **New `runtime_attempt_warm` path** in `src/bin/bench.rs`: one
  `RuntimeClient` is spawned via `RuntimeClient::spawn_in` a single time
  before the golden-task loop (see the new `warm_client_handshake` /
  `runtime_handshake` entry in the report), and that same client is reused
  for every task's warm read across all repeats — no re-spawn, no repeated
  `initialize`/`notifications/initialized` handshake. This directly answers
  the "no warm-client-reuse" gap called out in the original
  [Scope and what's blocked](#scope-and-whats-blocked) section. If the
  handshake itself fails (as it does on this machine, see below), that single
  failure is cached and returned instantly for every subsequent warm call
  instead of re-attempting the expensive handshake — this is measured and
  reported honestly rather than skipped.
- **Token approximation**: `approx_tokens` (bytes/4, `BYTES_PER_APPROX_TOKEN`
  in `src/lib.rs`) already existed from the original PR and is unchanged;
  this update did not need a new heuristic, just confirmed it still populates
  for any path that returns real content.

### Real, measured numbers (N=3, this run)

Command: `./target/release/simplicio-bench.exe --repeats 3 --out report.json`
(release build, Windows, this machine, Simplicio Runtime CLI 3.5.2 on
`PATH`). Full report:
`crates/codegen/simplicio-perf-bench/baselines/run_2026-07-17-repeats3-warmcold.json`.

**Handshake (paid once for the whole warm run):** `warm_client_handshake`
took **31.80s** and failed with the same MCP capability gap as the cold path
(`invalid Simplicio Runtime response: expected ',' or ']' at line 1 column
10181` — the malformed-JSON issue tracked as Runtime #3298, same as the
original N=1 run). `success_rate: 0.0` for the handshake itself.

| Task | Path | p50 (ms) | p95 (ms) | mean (ms) | stdev (ms) | success_rate |
|---|---|---:|---:|---:|---:|---:|
| `read_small` | `direct_read` | 0.23 | 16.43 | 5.62 | 9.35 | 1.0 |
| `read_small` | `runtime_attempt` (cold) | 22,186 | 26,101 | 23,351 | 2,391 | 0.0 |
| `read_small` | `runtime_attempt_warm` | 0.0000 | 0.0005 | 0.00017 | 0.00029 | 0.0 |
| `read_medium` | `direct_read` | 0.24 | 14.79 | 5.07 | 8.42 | 1.0 |
| `read_medium` | `runtime_attempt` (cold) | 14,355 | 23,339 | 17,336 | 5,199 | 0.0 |
| `read_medium` | `runtime_attempt_warm` | 0.0001 | 0.0003 | 0.00017 | 0.00012 | 0.0 |
| `read_monorepo_nested` | `direct_read` | 0.19 | 14.87 | 5.07 | 8.49 | 1.0 |
| `read_monorepo_nested` | `runtime_attempt` (cold) | 13,919 | 18,475 | 13,650 | 4,964 | 0.0 |
| `read_monorepo_nested` | `runtime_attempt_warm` | 0.0000 | 0.0003 | 0.0001 | 0.00017 | 0.0 |
| `read_large_synthetic` (4MiB) | `direct_read` | 2.09 | 2.16 | 2.11 | 0.04 | 1.0 |
| `read_large_synthetic` (4MiB) | `runtime_attempt` (cold) | 28,324 | 29,921 | 28,728 | 1,050 | 0.0 |
| `read_large_synthetic` (4MiB) | `runtime_attempt_warm` | 0.0001 | 0.0004 | 0.0002 | 0.00017 | 0.0 |
| `read_invalid_path` | `direct_read` | 0.016 | 0.062 | 0.031 | 0.027 | 0.0 (expected fail) |
| `read_invalid_path` | `runtime_attempt` (cold) | 29,073 | 32,158 | 29,703 | 2,208 | 0.0 |
| `read_invalid_path` | `runtime_attempt_warm` | 0.0001 | 0.0004 | 0.0002 | 0.00017 | 0.0 |

### What this run actually shows (and doesn't)

- **`direct_read` p50/p95/stdev across 3 repeats** is real, reproducible
  variance data (previously only a single point estimate): sub-millisecond to
  low-double-digit-millisecond, consistent with the N=1 run's order of
  magnitude.
- **Cold `runtime_attempt` is highly variable across only 3 repeats**: p50
  ranges 13.9s-29.1s and stdev is 1.0-5.2 *seconds* — i.e. the variance
  between repeats is itself on the order of the signal. With only 3 samples
  this is expected; more repeats would be needed to trust these percentiles
  as anything but a rough order-of-magnitude ("multiple tens of seconds per
  cold attempt on this machine, on this Runtime build").
- **Warm reuse changes latency by roughly 5 orders of magnitude per call**
  (tens of seconds -> sub-millisecond) **once the one-time handshake cost is
  already paid** — but on this machine the handshake itself never succeeds
  (same MCP capability gap as the original N=1 run), so this is a real,
  measured demonstration of *architecture*, not of a working warm Runtime
  path. It shows that reusing a `RuntimeClient` avoids re-paying spawn+
  handshake cost per call (cached failure returns near-instantly instead of
  re-attempting a ~15-30s handshake every time) — the same mechanical benefit
  would apply once the handshake actually succeeds, but that has not been
  observed on any run to date. Do not read the warm `success_rate: 0.0` rows
  as "Runtime reads succeeded quickly"; they mean "the cached failure was
  returned quickly."
- **Correctness**: identical to the N=1 run — `direct_read` produced all 5
  expected outcomes; no `runtime_attempt`/`runtime_attempt_warm` cell ever
  returned comparable content, so `content_matches_direct` and `approx_tokens`
  remain `null` for every Runtime-path row in this run, same as before.
- **Token counting on the Runtime path remains unexercised**: the
  bytes/4 `approx_tokens` heuristic only populates for a path/task combo that
  returns real content, and no Runtime call has returned content in any run
  captured in this document. There is nothing new to report on Runtime-side
  token counts until the MCP capability gap is fixed.

## What remains open

- **The CI-reproducible Runtime/Mapper-vs-direct-read comparison** —
  cold/warm/incremental, Mapper token savings, "concise mode" — blocked on a
  Runtime/Mapper implementation this repo builds or pins a known version of
  (tracked as #5/#6). The one real run captured above used whatever Runtime
  happened to be on a developer machine's `PATH`; it is not reproducible from
  a clean checkout of this repo and should not be treated as a committed
  baseline.
- **Root-causing the Runtime MCP capability failure** (malformed response in
  this run; `tools/list` timeout is tracked as Runtime #3298) before relying on
  any Runtime numbers. The 4MiB fixture path correctness issue is fixed here,
  but its Runtime result remains unverified until MCP is healthy — this is
  still true as of the 2026-07-17 N=3 update above; the same malformed-JSON
  error recurs verbatim.
- **A statistically meaningful sample.** The 2026-07-17 update raised this
  from N=1 to N=3 (real repeated measurements, not fabricated), which is
  enough to get a non-degenerate p50/p95/stdev but still far short of a
  trustworthy sample — cold-path stdev was itself 1-5 *seconds* across only 3
  repeats. N>=20 is still needed once the Runtime path is healthy enough that
  cold/warm numbers reflect successful reads, not a cached handshake failure.
- **Warm-vs-cold latency is now measured** (2026-07-17 update): reusing one
  `RuntimeClient` instead of re-spawning per call cut per-call overhead from
  ~14-30s to sub-millisecond in this run. But because the handshake itself
  never succeeds on this machine, this demonstrates the mechanism's overhead
  savings, not a validated warm-vs-cold comparison of successful Runtime
  reads. That comparison is still blocked on a healthy MCP handshake.
- **Wiring `simplicio-bench-check` into CI** against a pinned baseline on a
  dedicated runner.
- **Macro-level (whole-session, multi-tool-call) benchmarking** — this
  change only covers the single file-read primitive.
- **Token counting on a successful Runtime read** — the bytes/4 approximation
  exists and is wired to populate on any content-bearing response, but no run
  to date (N=1 or N=3) has ever gotten Runtime content to count tokens on.
