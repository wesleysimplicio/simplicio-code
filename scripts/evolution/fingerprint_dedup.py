#!/usr/bin/env python3
"""Fingerprint + taxonomy-hint classifier for evolution/finding signals.

Status: PROTOTYPE / LIBRARY-ONLY. Not wired into any execution path, CLI,
hook, or CI gate. Nothing in this module has mutation authority: it never
calls the GitHub API, never writes files, and never creates or updates an
issue. It exists to make one small, testable piece of issues #30 and #31
concrete ahead of the architectural review those RFCs ask for — see
`docs/rfc/continuous-evolution-coordinator.md`.

What this module does:

1. Normalizes a raw signal (component, symptom, expected, actual) into a
   stable, whitespace/case-insensitive string.
2. Computes a `fingerprint` (sha256 hex digest) from the normalized fields,
   so that the same underlying problem/opportunity observed multiple times
   (by different stages, agents, or runs) collapses to one identifier
   instead of spawning duplicate issues.
3. Groups a batch of signals by fingerprint (`dedupe`), returning which
   occurrences are duplicates of which canonical signal, plus an occurrence
   count that a real coordinator could use for prioritization.
4. Offers a *non-authoritative* taxonomy hint (`classify_hint`) that scores
   a signal against the 8-class taxonomy from issue #30
   (defect/regression/improvement/evolution/optimization/hardening/
   discovery/maintenance) using keyword heuristics, and always returns
   "discovery" when no class scores above the confidence floor — mirroring
   the RFC's gate rule that unproven hypotheses must never be presented as
   confirmed defects or improvements.

What this module deliberately does NOT do (left to a reviewed follow-up):
- decide anything about GitHub issue creation, ownership routing, or
  priority scoring against a budget;
- persist state across runs (no outbox, no SQLite, no ledger);
- accept untrusted issue/PR content as input without the caller sanitizing
  it first;
- claim to implement the full `simplicio.evolution-proposal/v1` contract —
  only the fingerprint + classification-hint fields are modeled here.

Run the tests: python3 scripts/tests/test_fingerprint_dedup.py
"""

from __future__ import annotations

import hashlib
import re
from dataclasses import dataclass, field
from typing import Iterable

# ---------------------------------------------------------------------------
# Taxonomy (kept in sync with docs/rfc/continuous-evolution-coordinator.md)
# ---------------------------------------------------------------------------

TAXONOMY_CLASSES = (
    "defect",
    "regression",
    "improvement",
    "evolution",
    "optimization",
    "hardening",
    "discovery",
    "maintenance",
)

# Minimum score (see _score_class) before a class hint is trusted over the
# safe default of "discovery". Deliberately conservative: the RFC's gate
# rule is "insufficient evidence => discovery", never a guessed class.
_CONFIDENCE_FLOOR = 2

# Small, explicit keyword sets per class. This is intentionally a coarse
# heuristic, not an ML classifier or an authoritative decision: it is meant
# to demonstrate the *shape* of a classify step, not to be trusted for
# auto-filing anything.
_CLASS_KEYWORDS: dict[str, tuple[str, ...]] = {
    "defect": ("violates", "contract broken", "throws", "panics", "crash", "incorrect output", "wrong result"),
    "regression": ("used to work", "previously passed", "was passing", "newly failing", "broke after", "reopen"),
    "improvement": ("could be better", "would be nicer", "improve", "enhance", "polish"),
    "evolution": ("new capability", "new architecture", "new contract", "rfc", "new stage", "new agent"),
    "optimization": ("slow", "latency", "token usage", "memory usage", "cpu", "reduce cost", "faster"),
    "hardening": ("security", "vulnerability", "resilience", "observability", "threat", "sandbox escape"),
    "discovery": ("might be", "unclear whether", "hypothesis", "investigate", "not yet confirmed", "suspected"),
    "maintenance": ("deprecated", "tech debt", "simplify", "remove dead code", "upgrade dependency", "cleanup"),
}

_WHITESPACE_RE = re.compile(r"\s+")


@dataclass(frozen=True)
class Signal:
    """A single observed signal, minimal subset of `simplicio.evolution-proposal/v1`.

    Only the fields needed for fingerprinting + a classification hint are
    modeled; a real implementation carries far more (run/task/stage/agent
    IDs, evidence refs, owner routing, priority score, etc. — see the RFC).
    """

    component: str
    symptom: str
    expected: str = ""
    actual: str = ""
    free_text: str = field(default="")

    def combined_text(self) -> str:
        return " ".join(
            part for part in (self.component, self.symptom, self.expected, self.actual, self.free_text) if part
        )


def normalize(text: str) -> str:
    """Lowercase, trim, and collapse internal whitespace for stable hashing."""
    return _WHITESPACE_RE.sub(" ", text.strip().lower())


def compute_fingerprint(signal: Signal) -> str:
    """Stable sha256 hex digest identifying the underlying problem/opportunity.

    Uses (component, symptom, expected, actual) — NOT free_text or any
    run/agent/timestamp field — so the same defect/opportunity reported by
    different stages, agents, or runs collapses to the same fingerprint.
    """
    parts = [normalize(signal.component), normalize(signal.symptom), normalize(signal.expected), normalize(signal.actual)]
    canonical = "\x1f".join(parts)  # unit-separator avoids accidental collisions across field boundaries
    return hashlib.sha256(canonical.encode("utf-8")).hexdigest()


@dataclass(frozen=True)
class DedupGroup:
    fingerprint: str
    canonical_index: int
    occurrence_indices: tuple[int, ...]

    @property
    def occurrence_count(self) -> int:
        return len(self.occurrence_indices)


def dedupe(signals: Iterable[Signal]) -> list[DedupGroup]:
    """Group signals by fingerprint.

    The first signal seen with a given fingerprint becomes the "canonical"
    occurrence (mirrors the RFC rule: update the canonical issue, don't
    create a new one). Returns groups in first-seen order.
    """
    order: list[str] = []
    groups: dict[str, list[int]] = {}
    for idx, signal in enumerate(signals):
        fp = compute_fingerprint(signal)
        if fp not in groups:
            groups[fp] = []
            order.append(fp)
        groups[fp].append(idx)

    return [
        DedupGroup(fingerprint=fp, canonical_index=groups[fp][0], occurrence_indices=tuple(groups[fp]))
        for fp in order
    ]


def _score_class(cls: str, normalized_text: str) -> int:
    return sum(1 for kw in _CLASS_KEYWORDS[cls] if kw in normalized_text)


@dataclass(frozen=True)
class ClassificationHint:
    class_hint: str
    confidence: int
    scored_classes: tuple[tuple[str, int], ...]
    authoritative: bool = False  # always False: this is a hint, never a decision


def classify_hint(signal: Signal) -> ClassificationHint:
    """Return a non-authoritative taxonomy hint for a signal.

    Never returns anything but "discovery" when no class clears the
    confidence floor — a caller MUST treat a low-confidence hint as
    "insufficient evidence", per the RFC's creation gate, not as a
    classification it can act on.
    """
    text = normalize(signal.combined_text())
    scored = tuple(sorted(((cls, _score_class(cls, text)) for cls in TAXONOMY_CLASSES), key=lambda kv: -kv[1]))
    best_class, best_score = scored[0]

    if best_score < _CONFIDENCE_FLOOR:
        return ClassificationHint(class_hint="discovery", confidence=best_score, scored_classes=scored)

    # Tie-break: if two classes score equally, prefer the safer ordering
    # defect > regression > hardening > ... > discovery so an ambiguous
    # signal is never quietly downgraded into "improvement" territory
    # (mirrors the RFC's "never label a defect as improvement" invariant).
    tied = [cls for cls, score in scored if score == best_score]
    if len(tied) > 1:
        for preferred in TAXONOMY_CLASSES:
            if preferred in tied:
                best_class = preferred
                break

    return ClassificationHint(class_hint=best_class, confidence=best_score, scored_classes=scored)


__all__ = [
    "TAXONOMY_CLASSES",
    "Signal",
    "DedupGroup",
    "ClassificationHint",
    "normalize",
    "compute_fingerprint",
    "dedupe",
    "classify_hint",
]
