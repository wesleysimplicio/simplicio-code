# RFC: Adaptive Architecture Coordinator (issue #31)

Status: **draft / design-stage — not implemented**
Tracks: [#31](https://github.com/wesleysimplicio/simplicio-code/issues/31)
Adapts: [simplicio-loop#468](https://github.com/wesleysimplicio/simplicio-loop/issues/468)
Depends on: [RFC: Continuous Evolution Coordinator](continuous-evolution-coordinator.md) (issue #30) — issue #31
explicitly states it depends on that sibling capability
Related: `crates/codegen/xai-grok-shell/src/agent/subagent/` (`SubagentCoordinator`) — see
[Naming](#naming-and-collision-avoidance) below

## Why this is an RFC and not a PR with code

Issue #31 asks for a coordinator that can detect gaps in Simplicio Code's
own workflow topology (intake, mapping, planning, execution, validation,
review, approval, checkpoint/recovery, delivery) and **propose** — with a
full governance lifecycle — new stages, new specialized agent roles,
splits/merges/reorders of stages, new or strengthened gates, and adjustments
to activation/dependency/retry/timeout/capacity/isolation. The proposal
lifecycle it specifies is: `topology_gap_observed → evidence/baseline →
dedup + owner → RFC/issue → semantic DAG diff → static validation →
simulation + historical replay → shadow run → independent approval →
versioned candidate → canary → compare → promote | revise | rollback`. It
requires an approval matrix by risk tier (low/medium/high/critical), a rule
that no proposer approves its own change, versioned/content-addressed
manifests, canary rollout with a comparator and kill switch, and a
Definition of Done that includes *actually proposing and running* a new
stage, a new agent, a simple adjustment, and a deliberately adversarial
attempt to weaken safety — end to end, in a sandbox.

This is, honestly, a self-modifying-agent-topology governance system with
real teeth: it can end in a *different* set of agents/stages actually
running in production, gated by an approval process that must resist the
system approving itself. Implementing this for real in one session — DAG
differ, static validator, capacity/cost simulator, historical replay,
shadow-run harness, approval-matrix enforcement, content-addressed manifest
versioning, canary comparator, kill switch, rollback — and then claiming a
"Definition of Done integrated run" actually happened, would be fabrication.
No such run occurred; no such subsystem exists yet. This RFC instead gives
the concrete design, an honest phased path, and the safety analysis a human
reviewer needs before any of this is allowed to touch a manifest that
controls what actually runs.

## What already exists in this repo

- **No existing topology/DAG/manifest coordinator.** Confirmed by repo-wide
  search: no "topology", no DAG-diff tooling, no stage-manifest concept in
  Simplicio Code today. The closest thing — `SubagentCoordinator` in
  `xai-grok-shell/src/agent/subagent/` — manages *live process lifecycle*
  for subagents within a single running session (spawn, cancel, query). It
  has no concept of a persisted, versioned topology manifest, no DAG, no
  approval matrix, and does not modify what stages/roles exist across runs.
- **simplicio-loop already owns this concept at the ecosystem level.**
  Issue #468 (this RFC's source) states plainly: "`simplicio-loop` já
  possui manifesto, coordinator, papéis e receipts por etapa" — i.e. the
  manifest/coordinator/role/receipt primitives this RFC would need already
  exist upstream, in the *loop*, not in Code. Issue #31 itself frames this
  as extending `simplicio-loop`'s stage-agent architecture (issues #422-433)
  to also cover Code's *own* internal workflow.
- **Petgraph is already a workspace dependency** (`petgraph = { version =
  "0.6.5", ... "stable_graph" }` in the root `Cargo.toml`), which is
  directly relevant: a real DAG differ/validator (cycle detection, orphan
  detection, reachability) should almost certainly be built on `petgraph`
  rather than a hand-rolled graph structure, and does not need a new
  external dependency.
- **`docs/ARCHITECTURE.md`** frames Code's own execution as TUI/headless/ACP
  → `SimplicioRuntimeFs` → MCP → Runtime, a straight client-of-Runtime
  relationship — there is currently no concept of Code having a
  swappable/versioned internal stage topology *at all*. Introducing one is
  a genuine architectural expansion, which is exactly why #31 correctly
  frames it as "autoarquitetura governada, não automodificação silenciosa"
  (governed self-architecture, not silent self-modification) and requires
  human/owner approval for anything above `low` risk.

## Naming and collision avoidance

To avoid confusion with the existing `SubagentCoordinator` (which manages
live subagent *processes* within a session), this RFC proposes calling the
component described here the **Topology Change Proposer** (TCP — name
collides with the network protocol acronym, so a better candidate is
**Workflow Topology Advisor**, WTA). It never has direct authority to spawn
or cancel anything; it only ever produces a `simplicio.topology-change/v1`
proposal artifact for external review. This is a deliberate naming and
responsibility split, not a synonym for the existing coordinator.

## Scope boundary with simplicio-loop

Same reasoning as the sibling RFC (#30): `simplicio-loop` already has the
manifest/coordinator/receipt primitives this RFC assumes exist. Two
reasonable architectures:

- **(A) Loop-owned:** Simplicio Code's workflow topology (its own internal
  stages: intake, map, plan, implement, validate, review, approve,
  checkpoint, deliver) is represented as data the *loop* manages centrally,
  and Code just executes whatever topology version it's pinned to for a
  given run. Gap-detection and proposal generation happen loop-side, fed by
  signals Code emits (reusing the Evolution Signal Emitter from the sibling
  RFC).
- **(B) Code-owned:** Code maintains its own topology manifest locally
  (its stage graph is arguably different in shape from the loop's
  cross-repo stage graph — Code's stages are things like "MCP handshake,"
  "sandboxed read/write," "checkpoint/rewind," which are Code-internal
  concerns the loop doesn't model at that granularity) and only *reports*
  gap observations and proposals outward, with `simplicio-loop` or a human
  approving.

This RFC leans toward **(B)** for the topology *representation* (because
Code's internal stage graph is genuinely different from the loop's,
concretely visible in `xai-grok-workspace`'s existing turn-boundary/
checkpoint machinery) combined with **(A)** for governance (approval,
canary decision, and audit trail live where humans already review
`simplicio-loop` activity). This is a recommendation for review, not a
decision — it directly affects how much new infrastructure Code needs to
build versus reuse.

## Contract: `simplicio.topology-change/v1`

```jsonc
{
  "change_id": "uuid-v4",
  "fingerprint": "sha256 — same signal from multiple observers collapses",
  "change_type": "add_stage|remove_stage|split_stage|merge_stage|reorder_stage|add_role|change_role|change_activation|change_dependency|change_isolation|add_gate|strengthen_gate|weaken_gate|change_retry|change_timeout|change_capacity|change_wave|add_adapter|add_receipt|add_subflow|deprecate_component",
  "risk": "low|medium|high|critical",
  "gap_evidence": { "observations": "integer", "baseline": "string", "refs": ["string"] },
  "current_topology_hash": "content hash of the manifest this proposal diffs against",
  "proposed_topology_hash": "content hash of the candidate manifest",
  "dag_diff": { "added_nodes": [], "removed_nodes": [], "changed_edges": [] },
  "affected_stages": ["string"],
  "affected_roles": ["string"],
  "affected_gates": ["string"],
  "authority_impact": "string — what mutation authority changes, if any",
  "isolation_impact": "string",
  "security_impact": "string",
  "surfaces": ["tui", "headless", "acp", "workspace"],
  "cost_estimate": { "tokens": "number", "latency_ms": "number", "slots": "number" },
  "alternatives_considered": ["string"],
  "migration_plan": "string",
  "coexistence_plan": "string — how old and new topology versions coexist during rollout",
  "shadow_plan": "string",
  "canary_plan": "string",
  "kill_switch": "string — concrete rollback trigger and mechanism",
  "promotion_metrics": [{ "metric": "string", "threshold": "string" }],
  "approval_class": "low|medium|high|critical",
  "approver": "string|null — MUST NOT equal proposer or coordinator itself",
  "state": "gap_observed|evidenced|deduped|rfc_open|validated|simulated|shadowed|approved|canarying|promoted|reverted|rejected",
  "owner_repo": "string",
  "created_at": "RFC3339"
}
```

## Requirements for a new stage / new role (reproduced from #31)

**New stage** must demonstrate: an independent boundary, its own
input/output/receipt, an objective activation and success condition, an
owner and authority, dependencies and a compensation path, required
isolation, proof that an existing hook/policy/check/adjustment is
insufficient, an estimated cost (slots/tokens/latency), and a fallback and
rollback plan.

**New role/agent** must demonstrate: a single mission that doesn't
duplicate an existing role, minimal capabilities, explicitly prohibited
tools and mutation authority, a minimal context pack, input/output schemas,
a receipt/completion condition, required independence/separation-of-duties,
runtime/model requirements and fallback, an objective activation condition,
measurable cost/benefit, a full create/ready/run/cancel/terminal lifecycle,
and responsibility for its own reports/findings.

## Validation and simulation pipeline (before any promotion)

1. Schema and reference validation.
2. Cycle/orphan/unreachable-node detection — this is precisely what
   `petgraph::algo` (already a workspace dependency) is built for
   (`is_cyclic_directed`, reachability via `Dfs`/`toposort`).
3. Mandatory-gate reachability for every activation path.
4. Capability and isolation validation.
5. Critical-path/capacity/wave/cost simulation.
6. Success/failure/retry/cancel/recovery simulation.
7. Replay against historical receipts/runs.
8. Shadow run with **no mutation authority** (observes only).
9. Independent adversarial review (a different actor than the proposer).
10. Before/after comparison.

## Approval matrix (reproduced from #31)

| Risk | Requirement |
|---|---|
| low | Parameters within pre-approved policy |
| medium | Optional/read-only stage or role; independent architectural review |
| high | Mutation tool, delivery, dependency, or isolation change; human/owner approval |
| critical | Weakens safety/auth/review/audit/recovery; multiple approvals; **never** auto-promoted |

Hard invariant: the proposer, any child agent it spawned, and the
coordinator itself **never** approve their own change. This must be
enforced structurally (e.g. the approval record's `approver` field is
validated against the proposal's `proposer` and rejected if equal), not
just documented as policy.

## Realistic phased rollout

| Phase | What | Buildable now? | Notes |
|---|---|---|---|
| 0 | This RFC | Yes — done | No code changes to any execution path. |
| 1 | A static, read-only representation of Code's *current* internal stage topology as data (not yet a DAG differ — just "what stages/roles exist today, expressed as a manifest") | Yes, next PR, human-reviewed | Pure documentation/data exercise. Zero mutation authority. Useful on its own as living architecture documentation, independent of anything else in this RFC. |
| 2 | A DAG validator (cycles/orphans/unreachable/mandatory-gate-reachability) over that static manifest, built on `petgraph`, with unit tests against known-good and known-bad graphs | Yes, needs review | Still read-only: validates a manifest, does not change what runs. |
| 3 | Gap-observation signals feeding into the Evolution Signal Emitter (issue #30's sibling RFC) with `change_type` classification, still producing proposals nobody consumes automatically | Yes, needs review | Reuses #30's fingerprint/dedup machinery instead of building a second one. |
| 4 | Semantic DAG diff (current manifest vs. candidate) and static validation of the diff | Needs org-level design review | First point where "candidate manifest" as a concept needs to exist and be content-addressed. |
| 5 | Capacity/cost simulator, historical replay, shadow-run harness | Needs org-level design review + real historical receipt data to replay against | Meaningful simulation needs production data Code doesn't have yet from a system that doesn't exist yet — chicken/egg that argues for building phases 1-4 first and observing them for a while. |
| 6 | Approval matrix enforcement + versioned/content-addressed manifest candidates via branch/PR | Needs org-level design review | This is where a real approval workflow (likely just: candidate topology change = a real PR requiring a human reviewer who isn't the proposer) starts to matter more than bespoke tooling. |
| 7 | Canary rollout, comparator, kill switch, promote/rollback | Needs org-level design review + a live population of runs to canary against | The highest-risk phase: this is the point where a topology change can actually alter what runs in production. Must not ship without a working kill switch verified *before* the first real canary. |
| 8 | Full sandbox Definition-of-Done run (propose new stage + new agent + simple adjustment + adversarial safety-weakening attempt, all the way through dedup/validate/simulate/shadow/canary/approve/promote/rollback) | Not before phases 1-7 are independently shipped and reviewed | Same reasoning as the sibling RFC: this is the last milestone, not a one-session deliverable. |

## Safety concerns and mitigations

| Concern | Why it matters | Mitigation |
|---|---|---|
| **Self-approval** | If the coordinator (or a child it spawned) can approve its own proposal, "governed self-architecture" degrades into silent self-modification — exactly what #31 says must never happen. | Structural invariant: approval record validation must reject `approver == proposer` and `approver == coordinator_id`. Should be a unit test from Phase 6 onward, not a policy note. |
| **Silent architecture mutation** | A topology change applied without going through diff/validate/simulate/shadow/canary could break activation paths or remove a safety gate invisibly. | Every phase above 3 is explicitly read-only/proposal-only until Phase 6's approval gate exists. No phase in this rollout has authority to change what actually executes. |
| **Auto-scope-expansion via "just a simple adjustment"** | Small parameter tweaks (retry counts, timeouts) are the easiest place for scope to creep from "propose" to "just apply it, it's low risk." | Even `low` risk changes stay within "pre-approved policy" per the approval matrix — meaning the *policy* is human-authored ahead of time, not decided by the coordinator in the moment. |
| **Weakening safety/review/audit silently** | The single most dangerous failure mode explicitly named in #31: a `weaken_gate` or `change_isolation` proposal that quietly reduces oversight. | These change types are hard-coded as `critical` risk requiring multiple independent approvals and are explicitly excluded from ever being auto-promoted, per the approval matrix above. The Phase 8 DoD's "deliberate adversarial attempt to weaken safety" exists specifically to prove this holds — that test must exist and must fail-closed before Phase 7 is considered done. |
| **Manifest drift / TOCTOU between approval and promotion** | If the manifest can change between when a human approves it and when it's promoted, the approval is meaningless. | Content-addressed, immutable manifest hashes (per #31): approval is granted for a specific hash; promotion checks the hash matches; any drift invalidates the approval. |
| **Runs migrating mid-flight** | A run that started under topology v1 silently switching to v2 mid-execution could behave inconsistently or break receipts. | Explicit invariant carried over from #31: active runs stay pinned to the manifest hash captured at intake; only new runs/canaries pick up a new version. |
| **Unbounded recursion (coordinator creating coordinators)** | A system that can propose new agent roles could, in principle, propose another coordinator, which proposes another, etc. | Explicit complexity budget from #31: max stages/roles/children per task/wave, max subflow depth, and an explicit rule against unlimited coordinator-creates-coordinator recursion. Must be enforced as a hard cap in code, not just documented. |
| **Root Cargo.toml / crate boundary** | Same invariant as the sibling RFC. | Nothing implemented in this PR touches `Cargo.toml` or any crate; Phase 1's "static topology as data" should also avoid becoming a new crate until reviewed — a Markdown/YAML manifest under `docs/` is sufficient for that phase. |

## What's implemented in this PR

Nothing beyond this document. The sibling RFC (#30) contributes a small,
inert Python prototype (fingerprint + dedup + classify-hint) that this RFC's
Phase 3 would reuse; this document does not add its own code, because
nothing in the phased rollout above is safe to build without the manifest
representation from Phase 1 existing and being reviewed first — building a
DAG differ or simulator before there's an agreed-on manifest to diff against
would be effort spent on a nonexistent input.

## What is explicitly NOT implemented

- Any representation (data or code) of Code's stage topology as a
  versioned manifest.
- Any DAG diff, cycle/orphan/reachability validator.
- Any capacity/cost/failure simulator or historical replay.
- Any shadow-run harness.
- Any approval-matrix enforcement or self-approval rejection logic.
- Any content-addressed manifest versioning or candidate branching.
- Any canary mechanism, comparator, or kill switch.
- The `gaps|propose|diff|validate|simulate|shadow|canary|promote|rollback|doctor --json` CLI/API surface described in #31.
- The Phase 8 integrated Definition-of-Done run.

## Open questions for human/architectural review

1. Does Code's internal stage topology (MCP handshake, sandboxed
   read/write, checkpoint/rewind, TUI/headless/ACP parity) actually need
   *dynamic* adaptation, or would a much smaller "propose a change, open a
   PR against a static manifest file, human reviews and merges it like any
   other code change" process satisfy #31's intent without building a
   simulator/canary system at all? This RFC suspects the latter may be
   sufficient for Code specifically (as opposed to `simplicio-loop`, which
   genuinely orchestrates many concurrent cross-repo agents and has a
   stronger case for automated canarying).
   canarying).
2. Who owns approval authority for `high`/`critical` changes to Code's
   topology — a specific human, a role, or a review board? This RFC cannot
   answer that; it is an organizational decision.
3. Should Phase 1's static manifest live in `docs/` (documentation-only,
   lowest risk) or as a new schema-validated data file consumed by a build
   step? Recommend starting with `docs/` and only promoting to a consumed
   artifact once Phase 4+ actually needs to read it programmatically.
4. Given `simplicio-loop` already has a stage-agent manifest/coordinator
   (#422-433), should this RFC's Phase 1+ manifest simply be a Code-scoped
   *extension* of that existing format instead of a new one invented here?
   Needs input from whoever owns the loop-side manifest schema.
