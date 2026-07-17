#!/usr/bin/env python3
"""Unit/system test for the pre-commit secret gate (issue #9).

Proves scripts/git-hooks/pre-commit BLOCKS a commit whose staged diff contains a canary secret
(an AWS-style access key id, matching hooks/action_gate.py's SECRETS patterns) and ALLOWS a
commit with no secret. Builds a disposable scratch git repo so it never touches this repo's own
history. Run: python3 scripts/tests/test_precommit_secret_gate.py
"""
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
HOOK_SRC = os.path.join(REPO, "scripts", "git-hooks", "pre-commit")
ACTION_GATE_SRC = os.path.join(REPO, "hooks", "action_gate.py")


def _run(argv, cwd):
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True)


def _make_scratch_repo():
    scratch = tempfile.mkdtemp(prefix="precommit-secret-gate-test-")
    _run(["git", "init", "-q"], cwd=scratch)
    _run(["git", "config", "user.email", "test@example.com"], cwd=scratch)
    _run(["git", "config", "user.name", "test"], cwd=scratch)
    os.makedirs(os.path.join(scratch, "hooks"), exist_ok=True)
    shutil.copy(ACTION_GATE_SRC, os.path.join(scratch, "hooks", "action_gate.py"))
    hook_dest = os.path.join(scratch, ".git", "hooks", "pre-commit")
    shutil.copy(HOOK_SRC, hook_dest)
    os.chmod(hook_dest, 0o755)
    return scratch


def _commit(scratch, filename, content):
    with open(os.path.join(scratch, filename), "w", encoding="utf-8") as f:
        f.write(content)
    _run(["git", "add", filename], cwd=scratch)
    return _run(["git", "commit", "-m", "test commit"], cwd=scratch)


def main():
    checks = []

    def chk(name, cond):
        checks.append(bool(cond))
        print("  [%s] %s" % ("ok" if cond else "XX", name))

    scratch = _make_scratch_repo()
    try:
        # Canary secret: a syntactically valid AWS access key id shape, never a real credential.
        # The trailing pragma exempts THIS line (in this file's own diff) from the gate; the
        # string value written into the scratch repo's config.py below carries no such marker,
        # so the gate still has to catch it there for the assertions below to mean anything.
        canary = "AWS_ACCESS_KEY_ID = \"AKIAABCDEFGHIJKLMNOP\"\n"  # pragma: allowlist secret
        blocked = _commit(scratch, "config.py", canary)
        chk("blocks a commit containing a canary secret", blocked.returncode != 0)
        log = _run(["git", "log", "--oneline"], cwd=scratch)
        chk("no commit was created for the blocked attempt", log.stdout.strip() == "")

        # The blocked attempt above left config.py (with the canary secret) staged: unstage it
        # before proving a clean commit succeeds, or the leftover secret would block this one too.
        _run(["git", "reset"], cwd=scratch)

        clean = _commit(scratch, "readme.txt", "hello world, no secrets here\n")
        chk("allows a commit with no secret", clean.returncode == 0)
        log2 = _run(["git", "log", "--oneline"], cwd=scratch)
        chk("exactly one commit exists after the clean commit", len(log2.stdout.strip().splitlines()) == 1)
    finally:
        shutil.rmtree(scratch, ignore_errors=True)

    ok = all(checks)
    print("selftest: %s (%d/%d)" % ("PASS" if ok else "FAIL", sum(checks), len(checks)))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
