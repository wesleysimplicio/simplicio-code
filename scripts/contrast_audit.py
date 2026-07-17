#!/usr/bin/env python3
"""WCAG AA contrast audit for the Simplicio Brasil (GrokNight) default theme.

Issue #7 requires the default green/yellow theme to meet WCAG AA contrast
without breaking error/success/warning legibility. There was no automated
check for this — palette values in
crates/codegen/xai-grok-pager-render/src/theme/groknight.rs were previously
eyeballed. This script computes real relative-luminance contrast ratios
(the same math as WCAG 2.x) for every semantic color the theme defines
against both of its backgrounds, and fails if any text-bearing color drops
below 4.5:1 (WCAG AA, normal text).

The hex values below are transcribed from
crates/codegen/xai-grok-pager-render/src/theme/groknight.rs::palette and
Theme::groknight(); this script does not parse Rust, so a future palette
change must update both. Ideally this becomes a Rust unit test once the
workspace is buildable in this environment again (see docs/brand-audit.md —
the whole crates/codegen workspace requires a `protoc` binary this sandbox
does not have).

Usage:
    python3 scripts/contrast_audit.py
"""

from __future__ import annotations

import sys

AA_NORMAL_TEXT = 4.5
AA_UI_COMPONENT = 3.0


def srgb_to_linear(c: int) -> float:
    c_norm = c / 255.0
    return c_norm / 12.92 if c_norm <= 0.04045 else ((c_norm + 0.055) / 1.055) ** 2.4


def relative_luminance(hex_color: str) -> float:
    hex_color = hex_color.lstrip("#")
    r, g, b = (int(hex_color[i : i + 2], 16) for i in (0, 2, 4))
    rl, gl, bl = srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b)
    return 0.2126 * rl + 0.7152 * gl + 0.0722 * bl


def contrast_ratio(fg: str, bg: str) -> float:
    l1, l2 = relative_luminance(fg), relative_luminance(bg)
    lighter, darker = max(l1, l2), min(l1, l2)
    return (lighter + 0.05) / (darker + 0.05)


# groknight.rs::palette (dark base) — kept in sync manually, see module doc.
BG_TERMINAL = "#0a0a0a"  # palette::BG
BG_BASE = "#141414"  # palette::BG_STORM

# Semantic colors that render as foreground text/accents against the two
# backgrounds above, per Theme::groknight(). (name, hex, minimum required)
TEXT_BEARING_COLORS = [
    ("text_primary (FG)", "#e1e1e1", AA_NORMAL_TEXT),
    ("text_secondary (FG_DARK)", "#c8c8c8", AA_NORMAL_TEXT),
    ("accent_success (GREEN)", "#009739", AA_NORMAL_TEXT),
    ("accent_error (RED)", "#f7768e", AA_NORMAL_TEXT),
    ("warning/command (YELLOW)", "#ffdf00", AA_NORMAL_TEXT),
    ("accent_assistant/accent_running (GREEN1)", "#00c853", AA_NORMAL_TEXT),
    # accent_system/accent_skill/fuzzy_accent/accent_verify were BLUE
    # (#002776, the literal flag navy) until this audit — that measured
    # ~1.4:1 against both backgrounds, far under AA. They now use BLUE1.
    ("accent_system/accent_skill/fuzzy_accent/accent_verify (BLUE1)", "#3A95AB", AA_NORMAL_TEXT),
]

# The retired flag-navy is checked too, but only asserted to *fail* — this
# guards against someone reintroducing it as a text color by accident.
KNOWN_FAILING_IF_USED_AS_TEXT = [
    ("BLUE (#002776, decorative-only, not used as text)", "#002776"),
]


def main() -> int:
    ok = True
    print(f"[contrast-audit] backgrounds: bg_terminal={BG_TERMINAL} bg_base={BG_BASE}\n")

    for name, hex_color, minimum in TEXT_BEARING_COLORS:
        r_term = contrast_ratio(hex_color, BG_TERMINAL)
        r_base = contrast_ratio(hex_color, BG_BASE)
        worst = min(r_term, r_base)
        status = "OK" if worst >= minimum else "FAIL"
        if worst < minimum:
            ok = False
        print(
            f"  [{status}] {name}: bg_terminal={r_term:.2f}:1 bg_base={r_base:.2f}:1 "
            f"(need >= {minimum}:1)"
        )

    print()
    for name, hex_color in KNOWN_FAILING_IF_USED_AS_TEXT:
        r_term = contrast_ratio(hex_color, BG_TERMINAL)
        note = "confirmed unfit for text (as expected, not used for text)"
        print(f"  [INFO] {name}: bg_terminal={r_term:.2f}:1 — {note}")

    print()
    print("[contrast-audit] PASS" if ok else "[contrast-audit] FAIL")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
