#!/usr/bin/env python3
"""Brand audit scanner for Simplicio Code (issue #7).

Simplicio Code is a rebrand of an internal xAI "Grok" CLI fork (itself built
on the sst/opencode project). This script gives the rebrand two independent,
automatable checks instead of a one-off manual grep pass:

1. STRICT surfaces: a short list of files that are the first thing a human
   or a search engine sees (root README/CONTRIBUTING/SECURITY, docs/**/*.md,
   prompt templates). These must contain zero occurrences of the retired
   brand tokens outside of a small, explicit, rationale-carrying allowlist.
   Any new hit here is a hard failure.

2. REGRESSION GUARD over source: a full scan of *.rs string literals
   mentioning "Grok" or "xAI" is compared against a checked-in baseline
   count. The rebrand is a large, multi-week mechanical effort (~735
   occurrences across ~144 files at the time this script was written —
   see docs/brand-audit.md) that is explicitly out of scope for a single
   session. Instead of ignoring it, this script fails the build if that
   count ever goes *up*, so newly written code can't introduce fresh leaks
   even though the historical backlog isn't fixed yet. Fixing backlog items
   should lower BASELINE_STRING_LITERAL_COUNT here to lock in the gain.

Usage:
    python3 scripts/brand_audit.py            # human-readable report
    python3 scripts/brand_audit.py --ci        # quiet, exit code only

Exit code 0 = pass, 1 = a strict-surface leak or a source regression.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Tokens that identify the retired brand layers. Matched case-insensitively.
RETIRED_TOKENS = ["opencode", "xai", "grok"]

# Directories never scanned (build output, vendored history, VCS metadata).
EXCLUDED_DIR_NAMES = {".git", "target", "node_modules", ".cargo"}

# --- Tier 1: strict public-facing surfaces -----------------------------

STRICT_FILES = [
    "README.md",
    "CONTRIBUTING.md",
    "SECURITY.md",
]
STRICT_GLOBS = [
    "docs/**/*.md",
    "crates/codegen/xai-grok-agent/templates/*.md",
]

# Docs *about* the brand audit itself necessarily discuss the retired brand
# names by name (that's the point of the document) — exempt them wholesale
# rather than drowning the real allowlist in self-referential entries.
STRICT_EXEMPT_FILES = {
    "docs/brand-audit.md",
    "docs/mapper-context.md",
}

# (file, line-substring) pairs that are known-safe in strict surfaces and
# documented in docs/brand-audit.md. Keep this list short; every entry here
# is a deliberate exception, not a place to silence a real leak.
STRICT_ALLOWLIST = [
    ("CONTRIBUTING.md", "SpaceXAI develops this software internally"),
    (
        "docs/ARCHITECTURE.md",
        "Grok, OpenRouter, OpenCode Go/Zen ou outros provedores",
    ),
    # README "Repository layout" / build instructions reference the real,
    # on-disk crate directory names (e.g. crates/codegen/xai-grok-pager).
    # Renaming ~40 xai-grok-* crates is a large mechanical effort tracked as
    # an open item in docs/brand-audit.md, not something to silently hide by
    # rewriting the docs to describe paths that don't exist.
    ("README.md", "xai-grok-pager-bin"),
    ("README.md", "crates/codegen/xai-grok-pager"),
    ("README.md", "crates/codegen/xai-grok-shell"),
    ("README.md", "crates/codegen/xai-grok-tools"),
    ("README.md", "crates/codegen/xai-grok-workspace"),
    ("README.md", "xai-grok-config"),
    ("README.md", "THIRD_PARTY_NOTICES.md"),  # legal attribution file, real path
    ("README.md", "opencode"),  # upstream attribution / fork provenance notes
    # The in-context docs-lookup hint still points at the real runtime
    # config directory. `~/.grok/` is a functional path baked into the
    # installed Runtime and CLI (auth cache, docs, config) — renaming it is
    # a migration, not a text edit, and is tracked as an open item.
    ("crates/codegen/xai-grok-agent/templates/prompt.md", "~/.grok/"),
]


def is_excluded(path: Path) -> bool:
    return any(part in EXCLUDED_DIR_NAMES for part in path.parts)


def scan_strict_surfaces() -> list[str]:
    failures: list[str] = []
    files: list[Path] = []
    for rel in STRICT_FILES:
        candidate = REPO_ROOT / rel
        if candidate.is_file():
            files.append(candidate)
    for pattern in STRICT_GLOBS:
        files.extend(p for p in REPO_ROOT.glob(pattern) if p.is_file())

    token_re = re.compile("|".join(RETIRED_TOKENS), re.IGNORECASE)

    for path in files:
        if is_excluded(path.relative_to(REPO_ROOT)):
            continue
        rel_str = path.relative_to(REPO_ROOT).as_posix()
        if rel_str in STRICT_EXEMPT_FILES:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="strict")
        except UnicodeDecodeError:
            continue
        for lineno, line in enumerate(text.splitlines(), start=1):
            if not token_re.search(line):
                continue
            if any(
                rel_str == allow_file and allow_substr in line
                for allow_file, allow_substr in STRICT_ALLOWLIST
            ):
                continue
            failures.append(f"{rel_str}:{lineno}: {line.strip()}")
    return failures


# --- Tier 2: regression guard over Rust string literals ----------------

# Baseline recorded 2026-07-17 after fixing the three highest-signal public
# leaks (system-prompt-label default, prompt.md self-identity line, and the
# session-ready notification title). See docs/brand-audit.md for the full
# breakdown of what remains and why. Regenerate with:
#   python3 -c "import scripts.brand_audit as b; print(b.count_source_string_literals())"
BASELINE_STRING_LITERAL_COUNT = 2035

STRING_LITERAL_TOKEN_RE = re.compile(r'"[^"]*(?:Grok|xAI)[^"]*"')


def count_source_string_literals() -> int:
    count = 0
    for path in (REPO_ROOT / "crates").rglob("*.rs"):
        if is_excluded(path.relative_to(REPO_ROOT)):
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="strict")
        except UnicodeDecodeError:
            continue
        count += len(STRING_LITERAL_TOKEN_RE.findall(text))
    return count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--ci", action="store_true", help="quiet mode: only print failures"
    )
    args = parser.parse_args()

    ok = True

    strict_failures = scan_strict_surfaces()
    if strict_failures:
        ok = False
        print(f"[brand-audit] STRICT SURFACE LEAKS ({len(strict_failures)}):")
        for line in strict_failures:
            print(f"  {line}")
    elif not args.ci:
        print("[brand-audit] strict surfaces clean (README/CONTRIBUTING/SECURITY/docs/templates).")

    current_count = count_source_string_literals()
    if current_count > BASELINE_STRING_LITERAL_COUNT:
        ok = False
        print(
            f"[brand-audit] REGRESSION: {current_count} Grok/xAI string literals "
            f"in crates/**/*.rs, up from baseline {BASELINE_STRING_LITERAL_COUNT}. "
            "A new occurrence of the retired brand was introduced."
        )
    elif not args.ci:
        delta = BASELINE_STRING_LITERAL_COUNT - current_count
        note = f" ({delta} fewer than baseline — update BASELINE_STRING_LITERAL_COUNT to lock in the gain)" if delta else ""
        print(
            f"[brand-audit] source regression guard OK: {current_count} Grok/xAI "
            f"string literals (baseline {BASELINE_STRING_LITERAL_COUNT}){note}."
        )

    if ok and not args.ci:
        print("[brand-audit] PASS")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
