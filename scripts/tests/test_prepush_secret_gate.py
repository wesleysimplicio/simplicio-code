#!/usr/bin/env python3
"""Unit/system test for the pre-push secret gate (issue #9).

Proves scripts/git-hooks/pre-push BLOCKS a push whose range contains a canary secret and does
NOT run scripts/check.py's simplicio-loop-specific self-test suite (the mismatch fixed after
the first push attempt against the real simplicio-code repo blocked on unrelated failures).
Builds a disposable bare "origin" plus a disposable clone so it never touches this repo's own
history or its real remote. Run: python3 scripts/tests/test_prepush_secret_gate.py
"""
import os
import shutil
import subprocess
import sys
import tempfile

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
HOOK_SRC = os.path.join(REPO, "scripts", "git-hooks", "pre-push")
ACTION_GATE_SRC = os.path.join(REPO, "hooks", "action_gate.py")


def _run(argv, cwd):
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True)


def _make_repo_with_remote():
    root = tempfile.mkdtemp(prefix="prepush-secret-gate-test-")
    bare = os.path.join(root, "origin.git")
    work = os.path.join(root, "work")
    _run(["git", "init", "-q", "--bare", bare], cwd=root)
    _run(["git", "init", "-q", work], cwd=root)
    os.makedirs(work, exist_ok=True)
    _run(["git", "init", "-q"], cwd=work)
    _run(["git", "config", "user.email", "test@example.com"], cwd=work)
    _run(["git", "config", "user.name", "test"], cwd=work)
    _run(["git", "remote", "add", "origin", bare], cwd=work)
    os.makedirs(os.path.join(work, "hooks"), exist_ok=True)
    shutil.copy(ACTION_GATE_SRC, os.path.join(work, "hooks", "action_gate.py"))
    # No scripts/check.py in this scratch repo at all -- proves the hook never depends on it.
    hook_dest = os.path.join(work, ".git", "hooks", "pre-push")
    shutil.copy(HOOK_SRC, hook_dest)
    os.chmod(hook_dest, 0o755)
    return root, work


def _commit(work, filename, content):
    with open(os.path.join(work, filename), "w", encoding="utf-8") as f:
        f.write(content)
    _run(["git", "add", filename], cwd=work)
    return _run(["git", "commit", "-m", "test commit"], cwd=work)


def main():
    checks = []

    def chk(name, cond):
        checks.append(bool(cond))
        print("  [%s] %s" % ("ok" if cond else "XX", name))

    root, work = _make_repo_with_remote()
    try:
        clean = _commit(work, "readme.txt", "hello world, no secrets here\n")
        chk("clean commit succeeds", clean.returncode == 0)
        push1 = _run(["git", "push", "-u", "origin", "HEAD:main"], cwd=work)
        chk("push with no secret in range is allowed", push1.returncode == 0)

        # A GitHub-token-shaped canary (never a real credential) matching action_gate's gh_ pattern.
        canary = "GITHUB_TOKEN = \"ghp_" + ("A" * 36) + "\"\n"  # pragma: allowlist secret
        _commit(work, "leak.py", canary)
        push2 = _run(["git", "push", "origin", "HEAD:main"], cwd=work)
        chk("push with a canary secret in range is blocked", push2.returncode != 0)
        chk("block reason mentions a secret",
            "secret" in (push2.stdout + push2.stderr).lower())

        # The bare remote must NOT have advanced past the first (clean) push.
        rev = _run(["git", "rev-parse", "origin/main"], cwd=work)
        head_after_clean = _run(["git", "rev-parse", "HEAD~1"], cwd=work)  # commit before the leak
        chk("blocked push never reached the remote",
            rev.returncode == 0 and rev.stdout.strip() == head_after_clean.stdout.strip())
    finally:
        shutil.rmtree(root, ignore_errors=True)

    ok = all(checks)
    print("selftest: %s (%d/%d)" % ("PASS" if ok else "FAIL", sum(checks), len(checks)))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
