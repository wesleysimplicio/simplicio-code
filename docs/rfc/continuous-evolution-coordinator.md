# RFC: Continuous Evolution Coordinator (issue #30)

Status: **draft / design-stage — not implemented**
Tracks: [#30](https://github.com/wesleysimplicio/simplicio-code/issues/30)
Adapts: [simplicio-loop#467](https://github.com/wesleysimplicio/simplicio-loop/issues/467), integrated with
[simplicio-loop#466](https://github.com/wesleysimplicio/simplicio-loop/issues/466)
Related: [RFC: Adaptive Architecture Coordinator](adaptive-architecture-coordinator.md) (issue #31)

This is the first document in `docs/rfc/` — there is no existing RFC convention
in this repo yet (checked: no `docs/rfc/`, `docs/adr/`, or `docs/proposals/`
directory, no RFC template referenced from `CONTRIBUTING.md`). This RFC
establishes the location and format as a byproduct; it does not assume that
choice is final.

## Why this is an RFC and not a PR with code

Issue #30 asks for a coordinator that observes signals across every stage of
Simplicio Code (TUI, headless, ACP, workspace), classifies them into an
8-class taxonomy, deduplicates against existing issues/PRs/RFCs across
multiple repositories, resolves the *owning* repository, opens or updates
deep GitHub issues with evidence and baselines, scores priority explicitly,
enforces creation budgets, and reports an "Evolution Ledger" per run — all
while never expanding the current task's scope and never silently degrading
safety. Its own Definition of Done calls for an *integrated run* that
injects defects and opportunities across multiple surfaces and proves all of
the above end-to-end.

That is a real subsystem: a schema, a state machine, a classifier, a
dedup/search integration against the GitHub API (and potentially other
repos' issue trackers), an outbox with retry/idempotency, a scoring and
budget engine, and a reporting layer wired into every stage of a
multi-surface Rust agent. Building and *safely verifying* that in one
session would mean one of two dishonest outcomes: a thin stub dressed up as
"done" with a fabricated integration run, or a hasty full implementation
with unreviewed classification heuristics deciding what becomes public
GitHub issues under this account. Neither is acceptable. What follows is the
honest alternative: a concrete design, a phased rollout that starts with
what is genuinely buildable now, and an explicit list of what remains
undone.

## What already exists in this repo (so this doesn't duplicate it)

- **No existing evolution/coordinator/RFC/receipt system.** A repo-wide
  search for "coordinator", "evolution", "RFC", "stage report", "receipt",
  "finding", and "topology" turned up nothing resembling this proposal.
- **Naming collision to avoid:** `crates/codegen/xai-grok-shell/src/agent/subagent/`
  already defines a `SubagentCoordinator` (`coordinator_lifecycle.rs`,
  `coordinator_query.rs`, `handle_request.rs`) that spawns, cancels, and
  queries live subagent processes during a single session. That is a
  runtime execution concern, unrelated to this RFC's meta-level "detect and
  file an issue about the system itself" concern. Any implementation of
  this RFC **must not** be named or structured so it is confused with that
  coordinator — see [Naming](#naming) below.
- **Durability precedent to reuse, not reinvent:** `xai-grok-workspace`
  already has a turn-boundary checkpoint mechanism
  (`session/checkpoint.rs`, `session/checkpoint_store.rs`) that persists
  state to a disk-backed store inside the sandboxed session root and a
  recovery path (`recovery.rs`). Any outbox/ledger persistence this RFC
  eventually needs should follow that pattern (sandboxed, durable,
  recoverable) rather than introducing a second persistence mechanism (e.g.
  a bespoke SQLite table duplicating what `xai-sqlite-journal` already
  offers for journaling).
- **Architecture constraint that shapes scope:** per `docs/ARCHITECTURE.md`,
  Simplicio Code is a *client* — TUI/headless/ACP talk to the Simplicio
  Runtime for all reads/writes/exec, and to the Gateway for model routing.
  Multi-stage orchestration across a *fleet* of agents/stages already lives
  in `simplicio-loop` (see #466–#468), which already has a manifest,
  coordinator, roles, and receipts for that purpose. This strongly suggests
  Simplicio Code's own responsibility here should be narrower than "replicate
  simplicio-loop's coordinator inside Code": Code should **emit** structured
  signals, not run its own duplicate coordinator loop. See
  [Scope boundary](#scope-boundary-with-simplicio-loop).

## Naming

To avoid confusion with `SubagentCoordinator`, this RFC proposes the name
**Evolution Signal Emitter** (ESE) for the Code-side component, reserving
"Continuous Evolution Coordinator" for the cross-repo aggregation role that
plausibly lives in `simplicio-loop` (or a shared crate consumed by both).
Code's job is to produce well-formed `simplicio.evolution-proposal/v1`
records; it does not need to itself decide GitHub issue routing across
`simplicio-mapper`, `simplicio-dev-cli`, and other repos — that
cross-repo ownership resolution is exactly what #466's `IssueTargetResolver`
already specifies at the ecosystem level.

## Scope boundary with simplicio-loop

Recommendation: **do not duplicate a full coordinator inside Simplicio Code.**
Instead:

1. Simplicio Code produces `simplicio.evolution-proposal/v1` signals locally
   (in-process, in the TUI/headless/ACP agent loop) whenever a stage
   observes something that plausibly belongs in one of the 8 taxonomy
   classes.
2. Signals are appended to a local, sandboxed ledger file (reusing the
   `xai-grok-workspace` checkpoint-store pattern for durability) — not
   published anywhere yet.
3. A separate, explicitly-invoked flush step (CLI subcommand or a
   `simplicio-loop`-side consumer) reads that ledger, does the actual
   dedup-against-GitHub, ownership resolution, and issue creation — reusing
   whatever `simplicio-loop` already builds for #466/#467 rather than
   re-implementing dedup/outbox/idempotency a second time in Rust inside
   Code.
4. If `simplicio-loop` cannot consume Code's ledger directly (different
   repo, different runtime), Code needs only a thin, well-tested exporter
   (e.g. `simplicio code evolution export --json`) — not a second full
   coordinator.

This keeps Code's part of the system small, keeps the taxonomy and dedup
logic in one place, and avoids two independently-evolving implementations of
the same contract drifting apart. If, after review, the architecture team
decides Code genuinely needs its own full coordinator (e.g. because
`simplicio-loop` cannot observe Code's internal stage transitions in
enough detail), that is itself the kind of decision this RFC says should go
through human review before code with GitHub-mutation authority is written.

## Contract: `simplicio.evolution-proposal/v1`

Below is the concrete schema this RFC proposes, in JSON Schema-ish shorthand
(a real implementation should generate this from `schemars`, matching the
crate's existing pattern in `crates/codegen/xai-grok-workspace-types`):

```jsonc
{
  "proposal_id": "uuid-v4",
  "fingerprint": "sha256 hex — stable across occurrences of the same signal",
  "run_id": "string",
  "task_id": "string",
  "stage": "intake|mapping|planning|execution|validation|review|delivery|watch",
  "agent_id": "string",
  "class": "defect|regression|improvement|evolution|optimization|hardening|discovery|maintenance",
  "component": "crate or subsystem name, e.g. xai-grok-sandbox",
  "owner_repo": "resolved-or-pending — see IssueTargetResolver in simplicio-loop#466",
  "current_limitation": "string",
  "evidence_refs": ["sanitized reference — never raw secrets/PII, see Safety"],
  "baseline": { "metric": "string", "value": "number|string" },
  "desired_outcome": "string",
  "expected_metrics": { "metric": "string", "target": "number|string" },
  "impact": "low|medium|high|critical",
  "blast_radius": "string",
  "frequency": "integer — occurrence count from dedup",
  "effort": "low|medium|high",
  "risk": "low|medium|high|critical",
  "confidence": "0.0-1.0",
  "dependencies": ["string"],
  "alternatives_considered": ["string"],
  "compatibility_notes": "string",
  "priority_score": "number — see Scoring",
  "priority_justification": "string",
  "issue_target": { "repo": "string|null", "url": "string|null" },
  "idempotency_key": "string",
  "remote_confirmation": "pending|confirmed|failed",
  "state": "observed|validated|linked|issue-created|deferred|rejected|delivered|regressed",
  "created_at": "RFC3339",
  "updated_at": "RFC3339"
}
```

This is a strict superset-compatible subset of what #467 asks for — every
field #467 lists appears here, grouped for Rust struct ergonomics. The
prototype module described in [What's implemented](#whats-implemented-in-this-pr)
below models only `component`, `symptom` (maps to `current_limitation`),
`expected`/`actual` (inputs to evidence), and a `class` hint — deliberately a
small slice of this contract, not the whole thing.

## Taxonomy and creation gate

Reproduced from issue #30 verbatim (Portuguese kept, since it is the
canonical source of truth the issue owner wrote):

| Classe | Condição | Tratamento |
|---|---|---|
| defect | viola contrato existente | issue obrigatória; pode bloquear completion |
| regression | comportamento anteriormente válido falhou | reabrir/vincular, revogar terminal quando necessário |
| improvement | aperfeiçoa capacidade existente | issue priorizada |
| evolution | nova capability/contrato/arquitetura | RFC/epic com rollout |
| optimization | reduz latência, tokens, CPU, RAM, I/O ou custo | baseline + meta + benchmark |
| hardening | aumenta segurança/resiliência/observabilidade | threat/failure model |
| discovery | hipótese ainda não comprovada | investigação, sem claim de implementação |
| maintenance | dívida, atualização, simplificação ou remoção | demonstrar custo/risco atual |

Before opening any issue, the gate requires proving: a concrete reproducible
opportunity, a benefited user/flow, absence of an equivalent existing
issue/PR/RFC, a correct owner, a measurable outcome, alignment with
ecosystem contracts, an incremental/testable/rollback-able strategy, and a
clear current-vs-future-scope boundary. Without sufficient evidence, the
signal is a `discovery`, never a stronger claim.

## Realistic phased rollout

| Phase | What | Buildable now? | Notes |
|---|---|---|---|
| 0 | This RFC + the fingerprint/classify-hint prototype (this PR) | Yes — done | Pure library, no GitHub calls, no mutation authority. See below. |
| 1 | In-process `Signal` emission at 2-3 real stage boundaries (e.g. validation failures, checkpoint recovery faults) writing to a local sandboxed ledger file (reuse `xai-grok-workspace` checkpoint-store durability pattern) | Yes, next PR, human-reviewed | No network access, no issue creation. Fully offline, fully testable. |
| 2 | `simplicio code evolution export --json` CLI exposing the local ledger for a `simplicio-loop`-side consumer to ingest | Yes, but needs CLI/UX review + a consumer on the other end | Establishes the Code/loop boundary described above. |
| 3 | Dedup-against-GitHub search + `IssueTargetResolver`-style owner routing | Needs org-level design review | Real GitHub API surface, rate limits, cross-repo permissions — security-sensitive (issue content becomes untrusted input on re-ingest). |
| 4 | Actual issue creation with outbox/retry/idempotency, budgets, and priority scoring | Needs org-level design review + staged rollout with a kill switch | This is where "coordinator" in the full sense from #30 actually starts mutating GitHub state. Must not ship without a human approval gate on the first N runs. |
| 5 | Full multi-repo integrated Definition-of-Done run as described in #30 | Not before phases 1-4 are independently shipped and observed in production for a real cycle | The issue's own DoD (inject defects/opportunities across surfaces, verify classify/dedup/create/budget/ledger end-to-end) is the right bar — it should be the *last* milestone, not a one-session deliverable. |

## Safety concerns and mitigations

| Concern | Why it matters | Mitigation |
|---|---|---|
| **Self-approval / auto-scope-expansion** | A coordinator that both detects an opportunity *and* decides to act on it during the same run can quietly expand the current task's blast radius. | Hard rule (already in #30): observing/proposing never enters the current working set. Phase 1-2 above literally cannot expand scope — they only write to a local ledger; nothing consumes it automatically. |
| **Fabricated confidence / false "confirmed"** | A heuristic classifier mislabeling a `discovery` as a `defect` becomes a false claim once it reaches a public issue tracker. | `classify_hint()` in the prototype is explicitly non-authoritative (`authoritative: False` always) and defaults to `discovery` below a confidence floor — see [What's implemented](#whats-implemented-in-this-pr). Any future phase that files a real issue must keep a human or a stronger, reviewed classifier in the loop before "confirmed" is claimed. |
| **Backlog explosion / issue spam** | Without dedup and budgets, N agents observing the same signal N times creates N issues. | Fingerprinting (implemented, see below) collapses duplicate occurrences before any issue-creation logic runs. Budgets and deferred/rejected memory are Phase 4 requirements, not optional. |
| **Secret/PII leakage into public issues** | Evidence refs and raw logs can contain tokens, signed URLs, or user data. | Explicitly out of scope until a sanitization pass is designed and reviewed (Phase 3+). The prototype takes no raw log input at all — it only operates on caller-supplied, already-abstracted `component`/`symptom`/`expected`/`actual` strings. |
| **Prompt injection via issue/PR content treated as trusted** | Dedup search results, existing issue bodies, and PR descriptions are external content and could contain instructions aimed at the agent (e.g. "mark this resolved", "reduce priority"). | Any Phase 3+ implementation must treat all fetched issue/PR/RFC content as untrusted data, never as instructions — consistent with this session's own operating rules. Must be an explicit, tested boundary, not an assumption. |
| **Cross-repo ownership mistakes** | Misrouting an issue to the wrong repo (or a repo without write access) either fails loudly or, worse, silently succeeds in the wrong place. | Reuse `IssueTargetResolver` design from simplicio-loop#466 rather than inventing a second one; fail closed (fallback issue in a known repo) rather than guessing. |
| **Root Cargo.toml / crate boundary violations** | Issue #30 explicitly states the generated root `Cargo.toml` stays read-only and crate boundaries must be respected. | This RFC's implemented slice is a standalone Python script under `scripts/evolution/`, not a new Cargo workspace member — it makes zero changes to `Cargo.toml` and has no dependency on any crate. Any future Rust implementation must add itself as a leaf crate without hand-editing the generated members list outside the normal codegen process. |

## What's implemented in this PR

A single, additive, non-wired-in Python module:

- **`scripts/evolution/fingerprint_dedup.py`** — implements exactly three
  things from the contract above, and nothing else:
  - `compute_fingerprint(signal)`: stable sha256 fingerprint from
    normalized `(component, symptom, expected, actual)`, ignoring
    whitespace/case and any free-text/run/agent metadata — the same
    underlying signal reported differently still collapses to one
    fingerprint.
  - `dedupe(signals)`: groups a batch of signals by fingerprint, returning
    the canonical (first-seen) occurrence and an occurrence count per
    group — the building block for "100 observações iguais geram uma
    issue" from #30's acceptance criteria.
  - `classify_hint(signal)`: a keyword-heuristic, explicitly
    **non-authoritative** taxonomy hint over the 8-class taxonomy, which
    always answers `discovery` when no class clears a confidence floor —
    mirroring the RFC's gate rule that unproven signals never get
    presented as confirmed defects or improvements.
- **`scripts/tests/test_fingerprint_dedup.py`** — 7 tests covering:
  fingerprint stability across case/whitespace/free-text variation,
  fingerprint distinctness for different signals, dedup grouping and
  occurrence counts, default-to-discovery behavior, keyword-based defect
  and optimization detection, and tie-break ordering. All pass:
  `python3 scripts/tests/test_fingerprint_dedup.py`.

This module:

- makes **no** GitHub API calls, has **no** network access, and writes
  **no** files;
- is **not imported or invoked** by any existing binary, hook, CI job, or
  CLI command in this repo — it is inert until a human wires it in after
  reviewing this RFC;
- does not touch `Cargo.toml` or any Rust crate;
- models only a slice of `simplicio.evolution-proposal/v1` (no run/task/
  owner/priority/state fields) — it is a prototype of the classify+dedup
  *logic*, not a partial implementation of the full coordinator.

## What is explicitly NOT implemented

- The full `simplicio.evolution-proposal/v1` struct/schema in Rust.
- Any GitHub search, dedup-against-live-issues, or `IssueTargetResolver`.
- Any issue creation, update, or comment posting.
- Priority scoring, budgets, or deferred/rejected memory.
- Any stage instrumentation inside `xai-grok-shell`, `xai-grok-workspace`,
  or any other crate.
- Evolution Ledger / Finding Ledger reporting in stage or final reports.
- Outbox, retry, or idempotent-confirmation machinery.
- The integrated multi-surface Definition of Done run described in #30.
- CLI/API surface (`gaps|propose|...` — that's actually issue #31's
  surface, not #30's, but neither exists here either).

## Open questions for human/architectural review

1. Should the Evolution Signal Emitter live in Simplicio Code at all, or
   should Code only ever emit raw stage events and let `simplicio-loop`
   own 100% of the classify/dedup/file-issue logic remotely? (This RFC
   leans toward "Code emits, loop coordinates," but that's a real
   architectural decision, not a foregone conclusion.)
2. What GitHub credentials/scopes would Phase 3+ use, and how are they kept
   out of the sandboxed agent process consistent with `xai-grok-secrets`'
   existing model?
3. Where does the local ledger live relative to the existing
   `xai-grok-workspace` checkpoint store — same sandboxed root, or a
   separate durability domain?
4. What's the actual creation budget (issues per run/period) and who sets
   it — hardcoded, config, or gateway-side policy?
