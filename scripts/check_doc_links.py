#!/usr/bin/env python3
"""Validate documentation links and referenced commands for Simplicio Code.

Checks, for every Markdown file rooted at the repo root (README.md,
CONTRIBUTING.md, SECURITY.md) and everything under `docs/`:

1. Relative file links (`[text](some/path.md)`, optionally with a `#anchor`)
   resolve to a file that actually exists on disk, relative to the linking
   file's directory.
2. `#anchor` links (same-file or cross-file) resolve to an actual Markdown
   heading in the target file, using GitHub's heading-to-slug algorithm.
3. `cargo <verb> -p <crate> ...` commands found in fenced code blocks
   reference a crate directory that actually exists under `crates/`.

External `http(s)://` links are only checked for well-formed syntax by
default (no network access assumed in CI). Pass `--check-external` to also
issue a HEAD/GET request to each unique external URL and flag non-2xx/3xx
responses; this is opt-in because it is slow and flaky in restricted network
environments.

Exit code is non-zero if any check fails. This is the "docs link checker"
required by issue "docs: onboarding beta, arquitetura e runbooks
operacionais" (acceptance criterion: "Todos os links e comandos são
verificados em CI").

Usage:
    python3 scripts/check_doc_links.py
    python3 scripts/check_doc_links.py --check-external
"""

from __future__ import annotations

import argparse
import re
import sys
import urllib.request
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parent.parent

# Root-level Markdown files that are part of the documentation surface.
ROOT_DOC_FILES = ["README.md", "CONTRIBUTING.md", "SECURITY.md"]

LINK_RE = re.compile(r"\[[^\]]*\]\(([^)]+)\)")
CODE_FENCE_RE = re.compile(r"^```")
HEADING_RE = re.compile(r"^(#{1,6})\s+(.*)$")
CARGO_PACKAGE_RE = re.compile(r"\bcargo\s+\S+.*?-p\s+([A-Za-z0-9_.\-]+)")


def slugify(heading: str) -> str:
    """Approximate GitHub's Markdown heading-to-anchor-slug algorithm."""
    text = heading.strip().lower()
    # Strip Markdown formatting markers and inline code backticks.
    text = re.sub(r"[`*_]", "", text)
    text = re.sub(r"[^\w\s-]", "", text)
    text = re.sub(r"\s+", "-", text)
    return text


def collect_doc_files() -> list[Path]:
    files = [REPO_ROOT / name for name in ROOT_DOC_FILES if (REPO_ROOT / name).is_file()]
    docs_dir = REPO_ROOT / "docs"
    if docs_dir.is_dir():
        files.extend(sorted(docs_dir.rglob("*.md")))
    # User-guide docs shipped with the pager crate (referenced from README).
    user_guide = REPO_ROOT / "crates/codegen/xai-grok-pager/docs"
    if user_guide.is_dir():
        files.extend(sorted(user_guide.rglob("*.md")))
    return files


def headings_in(path: Path) -> set[str]:
    slugs: set[str] = set()
    seen: dict[str, int] = {}
    try:
        text = path.read_text(encoding="utf-8")
    except OSError:
        return slugs
    in_fence = False
    for line in text.splitlines():
        if CODE_FENCE_RE.match(line.strip()):
            in_fence = not in_fence
            continue
        if in_fence:
            continue
        m = HEADING_RE.match(line)
        if m:
            base = slugify(m.group(2))
            count = seen.get(base, 0)
            seen[base] = count + 1
            slugs.add(base if count == 0 else f"{base}-{count}")
    return slugs


def known_crate_dirs() -> set[str]:
    names: set[str] = set()
    for base in ("crates/codegen", "crates/common", "crates/build", "prod/mc"):
        d = REPO_ROOT / base
        if not d.is_dir():
            continue
        for child in d.iterdir():
            if child.is_dir():
                names.add(child.name)
    return names


def url_is_reachable(url: str, timeout: float = 8.0) -> tuple[bool, str]:
    req = urllib.request.Request(url, method="HEAD", headers={"User-Agent": "doc-link-check/1.0"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp:  # noqa: S310
            return 200 <= resp.status < 400, str(resp.status)
    except Exception as exc:  # noqa: BLE001 - report any failure, network or HTTP
        return False, str(exc)


def check_file(
    path: Path,
    all_files: set[Path],
    crate_names: set[str],
    errors: list[str],
    external_links: set[str],
) -> None:
    text = path.read_text(encoding="utf-8")
    my_headings = headings_in(path)

    in_fence = False
    for lineno, line in enumerate(text.splitlines(), start=1):
        if CODE_FENCE_RE.match(line.strip()):
            in_fence = not in_fence
            continue

        if in_fence:
            for m in CARGO_PACKAGE_RE.finditer(line):
                crate = m.group(1)
                if crate not in crate_names:
                    errors.append(
                        f"{path.relative_to(REPO_ROOT)}:{lineno}: "
                        f"`cargo ... -p {crate}` references a crate that does not "
                        "exist under crates/"
                    )
            continue

        for m in LINK_RE.finditer(line):
            target = m.group(1).strip()
            if target.startswith("<") and target.endswith(">"):
                target = target[1:-1]
            if not target or target.startswith("mailto:"):
                continue
            if target.startswith("http://") or target.startswith("https://"):
                external_links.add(target)
                continue

            file_part, _, anchor = target.partition("#")
            if file_part == "":
                # Same-file anchor, e.g. [foo](#foo).
                if anchor and slugify(anchor.replace("-", " ")) not in my_headings and anchor not in my_headings:
                    errors.append(
                        f"{path.relative_to(REPO_ROOT)}:{lineno}: "
                        f"anchor '#{anchor}' not found as a heading in this file"
                    )
                continue

            resolved = (path.parent / file_part).resolve()
            if resolved not in all_files and not resolved.exists():
                errors.append(
                    f"{path.relative_to(REPO_ROOT)}:{lineno}: "
                    f"link target '{file_part}' does not exist "
                    f"(resolved: {resolved.relative_to(REPO_ROOT) if REPO_ROOT in resolved.parents else resolved})"
                )
                continue

            if anchor:
                target_headings = headings_in(resolved)
                if anchor not in target_headings:
                    errors.append(
                        f"{path.relative_to(REPO_ROOT)}:{lineno}: "
                        f"anchor '#{anchor}' not found in '{file_part}'"
                    )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check-external",
        action="store_true",
        help="Also fetch external http(s) links and flag non-2xx/3xx responses.",
    )
    args = parser.parse_args()

    doc_files = collect_doc_files()
    if not doc_files:
        print("no documentation files found", file=sys.stderr)
        return 1

    all_files = {f.resolve() for f in doc_files}
    crate_names = known_crate_dirs()

    errors: list[str] = []
    external_links: set[str] = set()
    for path in doc_files:
        check_file(path, all_files, crate_names, errors, external_links)

    if args.check_external:
        for url in sorted(external_links):
            ok, detail = url_is_reachable(url)
            if not ok:
                errors.append(f"external link unreachable: {url} ({detail})")

    print(f"checked {len(doc_files)} Markdown file(s), {len(external_links)} external link(s) seen")
    if errors:
        print(f"\n{len(errors)} problem(s) found:\n")
        for err in errors:
            print(f"  - {err}")
        return 1

    print("all documentation links and referenced cargo commands are valid")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
