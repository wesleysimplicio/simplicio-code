# Prototype-First preview and decision gate

The canonical receipt is `simplicio.prototype-decision/v1`, implemented as
data-only types in `xai-grok-workspace-types::prototype_first`. A receipt
identifies the plan and source revision, carries typed Runtime-owned preview
artifacts, evidence, assumptions, limitations, risk, cost, provenance, and
acceptance-criteria coverage.

The same receipt is rendered by the TUI, workspace/UI, headless, and ACP
adapters. JSON surfaces use `simplicio.prototype-preview/v1`; TUI uses a
bounded text fallback with explicit keyboard actions and pagination. Rendering
does not grant mutation authority.

`PrototypePanel` is the product-facing gallery controller shared by all four
surfaces. It supports candidate selection, type filters, side-by-side semantic
comparison, bounded paging, and evidence drill-down without loading artifact
bytes outside Runtime. `semantic_view` is the single presentation model: each
adapter adds only its `surface` tag, so state, decision, candidates, evidence,
limitations, risk, cost, AC coverage, and available actions cannot drift.

Decision actions are explicit human gates. Plan-authority panels are read-only;
they can browse and compare but cannot record a mutation. Accept always needs
confirmation, and medium/high/critical-risk revise or reject actions do too.
Cancelling confirmation leaves the receipt unchanged. A source-stale or
otherwise invalid receipt rejects every decision action until revalidation.

The gate is fail-closed:

- only `accept`, `revise`, and `reject` are valid decisions;
- `revise` and `reject` require an explanation;
- missing evidence, missing AC coverage, unsafe artifact references, invalid
  controls, malformed receipts, and source/plan drift block Build;
- recorded comparisons are recomputed from their two distinct candidates, so
  unknown IDs or forged `changed_fields` cannot enter an auditable receipt;
- URI validation decodes percent escapes and rejects POSIX/Windows traversal,
  malformed escapes, backslashes, and NUL bytes before an artifact is opened;
- Build requires a current `accept` and yields a
  `simplicio.build-authorization/v1` receipt;
- `RuntimeClient::{write_prototype_artifact, read_prototype_artifact}` are the
  only Code-side persistence and retrieval adapters for
  `.simplicio/artifacts/prototype-first`; they delegate to negotiated Runtime
  artifact capabilities;
- telemetry contains only digests, IDs, decision, state, and risk; it never
  contains prompt, code, artifact content, or secrets.

`scripts/validate_prototype_decision.py` mirrors the Rust validator for
repository/headless preflight and can render `--surface tui|ui|headless|acp`.
The contract fixture is
[`docs/contracts/prototype-first-decision.v1.json`](contracts/prototype-first-decision.v1.json).

External acceptance evidence is combined with
`scripts/validate_prototype_acceptance.py`. The deterministic receipt requires
Loop #568 to accept every state, a versioned negotiated Runtime binary with a
SHA-256 identity plus `simplicio_prototype_artifact_write` and
`simplicio_prototype_artifact_read`, identical plan/source
revisions, all four complete product surfaces, failure/rollback evidence, and
matching replay hashes. Missing upstream evidence remains explicitly
`blocked`; the validator never substitutes a mock or local filesystem result.
Malformed capability arrays, unknown or contradictory surface results, and
non-finite JSON measurements also fail closed, so a failed run cannot be
masked by appending a second successful result for the same surface.

```bash
python3 scripts/validate_prototype_acceptance.py \
  --loop /path/loop.json --runtime /path/runtime.json --e2e /path/e2e.json
```

For a real installed integration proof, run the provider-free harness below.
It starts the supplied Runtime binary over MCP stdio, writes and reads
Runtime-owned artifacts, injects an unsafe artifact ID, exercises Loop #568,
validates Code's decision gate, renders TUI/UI/headless/ACP, and writes a
single acceptance receipt. The command must receive a real Loop checkout and a
real executable Runtime binary; it does not use mocks, a local filesystem
fallback, DeepSeek, or any local LLM.

```bash
PYTHONPATH=/path/to/simplicio-loop \
python3 scripts/prototype_product_e2e.py \
  --repo /path/to/simplicio-code \
  --loop-root /path/to/simplicio-loop \
  --runtime /path/to/simplicio-runtime \
  --output /tmp/simplicio-prototype-product-e2e.json
```

The resulting `simplicio.prototype-product-e2e/v1` receipt is the evidence
used to close the Prototype-First acceptance issue. A passing validator-only
fixture is not a substitute for this installed binary run.

For the external Hub ownership proof, run the Code Rust transport against a
real Loop daemon from the merged Loop checkout:

```bash
python3 scripts/code_loop_hub_e2e.py \
  --repo /path/to/simplicio-code \
  --loop-root /path/to/simplicio-loop \
  --output /tmp/simplicio-code-loop-hub-e2e.json
```

This proof starts only Loop Hub. Code attaches through the versioned socket,
uses Hub-owned Runtime/Mapper/scheduler/inference handles, and exercises
submit/progress/cancel. It does not start a local scheduler, Runtime, Mapper,
worker, model, DeepSeek, or local LLM.

Use `--benchmark 10000` to measure the receipt-validation hot path. Timing is
otherwise `null` with a reason so a deterministic receipt never estimates an
unobserved metric.
