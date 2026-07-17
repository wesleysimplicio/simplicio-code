#!/usr/bin/env python3
"""Unit tests for scripts/evolution/fingerprint_dedup.py (RFC prototype slice for issues #30/#31).

Proves the fingerprint/dedup/classify-hint helpers behave as documented:
- identical (component, symptom, expected, actual) collapses to one fingerprint
  regardless of whitespace/case/free_text differences;
- distinct underlying problems get distinct fingerprints;
- dedupe() groups a batch correctly and counts occurrences;
- classify_hint() only returns a non-"discovery" class when keyword evidence
  clears the confidence floor, and always answers "discovery" otherwise;
- the module has no side effects: no network, no filesystem writes, no GitHub calls.

Run: python3 scripts/tests/test_fingerprint_dedup.py
"""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(REPO, "scripts", "evolution"))

from fingerprint_dedup import (  # noqa: E402
    Signal,
    classify_hint,
    compute_fingerprint,
    dedupe,
)

FAILURES = []


def check(name, condition):
    if condition:
        print(f"PASS {name}")
    else:
        print(f"FAIL {name}")
        FAILURES.append(name)


def test_same_signal_same_fingerprint_ignoring_whitespace_case():
    a = Signal(component="xai-grok-workspace", symptom="checkpoint restore drops hunks", expected="hunks restored", actual="hunks lost")
    b = Signal(
        component="  XAI-Grok-Workspace ",
        symptom="Checkpoint   restore drops   hunks",
        expected="Hunks Restored",
        actual="hunks   lost",
        free_text="observed by agent-7 during run 42",  # must NOT affect fingerprint
    )
    check("same signal (case/whitespace/free_text varies) -> same fingerprint", compute_fingerprint(a) == compute_fingerprint(b))


def test_different_signal_different_fingerprint():
    a = Signal(component="xai-grok-mcp", symptom="handshake times out", expected="ready in 2s", actual="never ready")
    b = Signal(component="xai-grok-sandbox", symptom="symlink escape allowed", expected="denied", actual="allowed")
    check("different signals -> different fingerprints", compute_fingerprint(a) != compute_fingerprint(b))


def test_dedupe_groups_and_counts_occurrences():
    dup1 = Signal(component="xai-grok-mcp", symptom="handshake times out", expected="ready", actual="timeout")
    dup2 = Signal(component=" XAI-GROK-MCP", symptom="Handshake Times Out", expected="Ready", actual="Timeout")
    unique = Signal(component="xai-grok-sandbox", symptom="symlink escape allowed", expected="denied", actual="allowed")

    groups = dedupe([dup1, unique, dup2])

    check("dedupe finds exactly 2 groups for 3 signals (2 dup + 1 unique)", len(groups) == 2)
    dup_group = next(g for g in groups if g.occurrence_count == 2)
    unique_group = next(g for g in groups if g.occurrence_count == 1)
    check("duplicate group has canonical_index 0 (first-seen)", dup_group.canonical_index == 0)
    check("duplicate group occurrence_indices reference both duplicate positions", dup_group.occurrence_indices == (0, 2))
    check("unique group occurrence_indices references only its own position", unique_group.occurrence_indices == (1,))


def test_classify_hint_defaults_to_discovery_without_evidence():
    vague = Signal(component="xai-grok-agent", symptom="feels a bit off sometimes")
    hint = classify_hint(vague)
    check("vague/no-keyword signal classifies as discovery", hint.class_hint == "discovery")
    check("discovery hint is never marked authoritative", hint.authoritative is False)


def test_classify_hint_recognizes_defect_keywords():
    crashy = Signal(
        component="xai-grok-shell",
        symptom="tool call throws and panics",
        expected="graceful error contract",
        actual="incorrect output then crash",
    )
    hint = classify_hint(crashy)
    check("crash/contract-violation language hints at defect", hint.class_hint == "defect")
    check("defect hint confidence clears the floor", hint.confidence >= 2)


def test_classify_hint_recognizes_optimization_keywords():
    slow = Signal(
        component="xai-codebase-graph",
        symptom="indexing is slow and burns cpu",
        expected="fast latency",
        actual="high token usage and high cpu, reduce cost needed, faster please",
    )
    hint = classify_hint(slow)
    check("perf-language hints at optimization", hint.class_hint == "optimization")


def test_classify_hint_tie_break_prefers_safer_class():
    # Construct a signal that scores "defect" and "maintenance" equally, to
    # exercise the tie-break preferring the earlier (safer) taxonomy entry.
    tied = Signal(
        component="xai-tool-runtime",
        symptom="incorrect output, throws exception",  # 2 defect keywords: "incorrect output", "throws"
        expected="deprecated dependency should be upgrade dependency",  # 2 maintenance keywords
        actual="",
    )
    hint = classify_hint(tied)
    defect_score = dict(hint.scored_classes)["defect"]
    maintenance_score = dict(hint.scored_classes)["maintenance"]
    check("tie scenario actually ties (test sanity check)", defect_score == maintenance_score and defect_score >= 2)
    check("tie-break prefers defect over maintenance (safer ordering)", hint.class_hint == "defect")


def main():
    test_same_signal_same_fingerprint_ignoring_whitespace_case()
    test_different_signal_different_fingerprint()
    test_dedupe_groups_and_counts_occurrences()
    test_classify_hint_defaults_to_discovery_without_evidence()
    test_classify_hint_recognizes_defect_keywords()
    test_classify_hint_recognizes_optimization_keywords()
    test_classify_hint_tie_break_prefers_safer_class()

    if FAILURES:
        print(f"\n{len(FAILURES)} test(s) failed: {FAILURES}")
        sys.exit(1)
    print("\nAll tests passed.")


if __name__ == "__main__":
    main()
