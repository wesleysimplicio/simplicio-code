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

The gate is fail-closed:

- only `accept`, `revise`, and `reject` are valid decisions;
- `revise` and `reject` require an explanation;
- missing evidence, missing AC coverage, unsafe artifact references, invalid
  controls, malformed receipts, and source/plan drift block Build;
- Build requires a current `accept` and yields a
  `simplicio.build-authorization/v1` receipt;
- `RuntimeClient::write_prototype_artifact` is the only Code-side persistence
  adapter for `.simplicio/artifacts/prototype-first`; it delegates to the
  negotiated Runtime write capability;
- telemetry contains only digests, IDs, decision, state, and risk; it never
  contains prompt, code, artifact content, or secrets.

`scripts/validate_prototype_decision.py` mirrors the Rust validator for
repository/headless preflight and can render `--surface tui|ui|headless|acp`.
The contract fixture is
[`docs/contracts/prototype-first-decision.v1.json`](contracts/prototype-first-decision.v1.json).

External acceptance evidence is combined with
`scripts/validate_prototype_acceptance.py`. The deterministic receipt requires
Loop #568 to accept every state, a versioned negotiated Runtime binary with a
SHA-256 identity and `simplicio_prototype_artifact_write`, identical plan/source
revisions, all four complete product surfaces, failure/rollback evidence, and
matching replay hashes. Missing upstream evidence remains explicitly
`blocked`; the validator never substitutes a mock or local filesystem result.

```bash
python3 scripts/validate_prototype_acceptance.py \
  --loop /path/loop.json --runtime /path/runtime.json --e2e /path/e2e.json
```

Use `--benchmark 10000` to measure the receipt-validation hot path. Timing is
otherwise `null` with a reason so a deterministic receipt never estimates an
unobserved metric.
