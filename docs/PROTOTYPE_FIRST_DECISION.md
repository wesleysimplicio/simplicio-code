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

The external Loop #568 state/reporting contract and a Runtime binary are not
available in this checkout. Code therefore exposes the typed state and keeps
the integration fail-closed; the full product E2E remains dependent on those
upstream surfaces.
