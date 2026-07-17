# Token / latency benchmark (issue #12)

Status: **partial**. This document covers what is measurable inside this repo
today, plus one real (small-sample) run against a Simplicio Runtime binary
that happened to be installed on the machine that produced this document. It
is not the full, CI-reproducible "Runtime cold/warm/incremental vs direct
read" comparison the issue asks for — see
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
   against whatever Runtime binary `resolve_binary()` finds.

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
*expected* to fail; the other four are *expected* to succeed for
`direct_read`). The harness reports `success_rate` per task/path so
correctness is checked before any token/latency number is trusted, per the
issue's acceptance criteria. `expected_success` for `runtime_attempt` is
deliberately not asserted against a fixed value in code (see
[Scope and what's blocked](#scope-and-whats-blocked)) — the observed
success/failure itself is the data point.

### Metrics

- **Latency**: wall-clock per read, `Instant::now()`/`elapsed()`, over N
  repeats per task (`--repeats`, default 20). Reported as p50, p95, mean, and
  sample standard deviation in milliseconds.
- **Tokens**: `approx_tokens = byte_len / 4.0`. A documented,
  dependency-free **approximation**, not a real model tokenizer count — see
  `BYTES_PER_APPROX_TOKEN` in `crates/codegen/simplicio-perf-bench/src/lib.rs`.
  Treat it as a relative trend line, not an absolute token count for any
  specific model.
- **Success rate**: fraction of repeats that returned the expected
  (`direct_read`) or observed (`runtime_attempt`) outcome.

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
(`./target/debug/simplicio-bench.exe --repeats 1`, debug build, Windows).
Full JSON: `crates/codegen/simplicio-perf-bench/baselines/run_2026-07-17.json`.

**Honesty note on sample size**: this is N=1 per cell, not the 20+ repeats
the harness defaults to and that a trustworthy p50/p95/stdev needs. Each
`runtime_attempt` call spawns a fresh process and does a full MCP handshake
against a real installed Runtime binary, which took 4–10 seconds *per call*
in this environment — running the default 20 repeats across all 5 tasks
(200 process spawns) did not complete in the time available for this change.
Treat every number below as a single anecdotal data point, not a trend.

| Task | Path | Success | Latency (ms) | approx tokens |
|---|---|---|---|---|
| `read_small` (~460B) | `direct_read` | 1/1 | 0.60 | 94.25 |
| `read_small` (~460B) | `runtime_attempt` | 1/1 | 6439.19 | n/a |
| `read_medium` (~10.6KB) | `direct_read` | 1/1 | 0.22 | 2217.5 |
| `read_medium` (~10.6KB) | `runtime_attempt` | 1/1 | 6198.29 | n/a |
| `read_monorepo_nested` | `direct_read` | 1/1 | 0.43 | 84.5 |
| `read_monorepo_nested` | `runtime_attempt` | 1/1 | 5020.66 | n/a |
| `read_large_synthetic` (4MiB) | `direct_read` | 1/1 | 2.01 | 1,048,576 |
| `read_large_synthetic` (4MiB) | `runtime_attempt` | 0/1 | 10457.63 | n/a |
| `read_invalid_path` | `direct_read` | 0/1 (expected) | 0.06 | n/a |
| `read_invalid_path` | `runtime_attempt` | 0/1 (expected) | 4456.98 | n/a |

Key takeaways from this run:

- **Correctness**: `direct_read` matched its expected outcome on all 5 tasks
  (succeeded on 4, correctly failed on the invalid path).
  `runtime_attempt` against the installed Runtime binary **succeeded** on
  `read_small`, `read_medium`, and `read_monorepo_nested` (a real Runtime did
  answer `simplicio_file_read` correctly for those three), and **failed** on
  `read_large_synthetic` and (expectedly) `read_invalid_path`. This repo's
  own `RuntimeClient` doesn't currently surface *why* the 4MiB read failed
  (only that it did); it's plausibly a size/config limit inside that
  specific installed Runtime build. That's worth root-causing before relying
  on it, but is out of scope for this benchmark change.
- **Latency**: every `direct_read` was sub-3ms; every `runtime_attempt` was
  4.4–10.5 **seconds** — three to four orders of magnitude slower. This is
  expected given the harness pays full process-spawn + MCP-handshake cost on
  every single call (no client reuse), so it is not a fair "steady-state"
  comparison — it's a worst-case cold-path number. A real cold/warm
  comparison (reusing one client across repeats, the way
  `SimplicioRuntimeFs` actually does in the agent) is exactly the
  [blocked work](#scope-and-whats-blocked) above.
- `approx_tokens` is only computed for `direct_read`, since that's the only
  path that returns content to the harness process today (the Runtime
  client's `read_file` result content isn't currently threaded back out of
  `main.rs` — a straightforward follow-up, not done here to keep this change
  scoped to latency + the correctness gate).

## What remains open

- **The CI-reproducible Runtime/Mapper-vs-direct-read comparison** —
  cold/warm/incremental, Mapper token savings, "concise mode" — blocked on a
  Runtime/Mapper implementation this repo builds or pins a known version of
  (tracked as #5/#6). The one real run captured above used whatever Runtime
  happened to be on a developer machine's `PATH`; it is not reproducible from
  a clean checkout of this repo and should not be treated as a committed
  baseline.
- **A statistically meaningful sample.** This change's committed baseline is
  N=1 per cell because 20-repeat runs did not finish in the available time
  (each `runtime_attempt` costs several seconds of real process-spawn +
  handshake time). Re-running `scripts/bench/run.sh --repeats 20` (or higher)
  on a machine with time to spare, and replacing
  `crates/codegen/simplicio-perf-bench/baselines/run_2026-07-17.json`, is the
  immediate next step before trusting p50/p95/stdev from this harness.
- **Root-causing the `read_large_synthetic` runtime_attempt failure** instead
  of just recording that it happened.
- **Threading Runtime response content back out** so `approx_tokens` can be
  computed for `runtime_attempt` too, enabling an actual token-savings
  comparison (today only latency and success/failure are comparable).
- **Wiring `simplicio-bench-check` into CI** against a pinned baseline on a
  dedicated runner.
- **Macro-level (whole-session, multi-tool-call) benchmarking** — this
  change only covers the single file-read primitive.
