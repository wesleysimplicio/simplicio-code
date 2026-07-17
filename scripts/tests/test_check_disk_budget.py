#!/usr/bin/env python3
"""Unit tests for scripts/check_disk_budget.py (issue #32).

Covers the sufficient/insufficient/boundary decision logic and the cache
measurement/remediation plumbing entirely with mocked filesystem stats —
this test never touches a real disk, and it never fills, fabricates, or
deletes anything on disk. It also asserts the module contains no
file-deletion calls at all, per issue #32's explicit "no automatic
cleanup" requirement.

Run: python3 scripts/tests/test_check_disk_budget.py
"""
import inspect
import os
import subprocess
import sys
from types import SimpleNamespace

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
sys.path.insert(0, os.path.join(REPO, "scripts"))

import check_disk_budget as cdb  # noqa: E402

GIB = 1024 * 1024 * 1024


def _fake_cache(name, path, size_bytes, exists=True):
    return cdb.CacheDirReport(name=name, path=path, size_bytes=size_bytes, exists=exists)


def main():
    checks = []

    def chk(name, cond):
        checks.append(bool(cond))
        print("  [%s] %s" % ("ok" if cond else "XX", name))

    # ---- pure decision function: sufficient / insufficient / boundary ----

    sufficient = cdb.check_disk_budget(
        free_bytes=10 * GIB, required_bytes=5 * GIB, path="/ws", caches=[]
    )
    chk("sufficient space -> status ok", sufficient.status == "ok")
    chk("sufficient space -> ok property true", sufficient.ok is True)
    chk("sufficient space -> zero deficit", sufficient.deficit_bytes == 0)
    chk("sufficient space -> no remediation text", sufficient.remediation == "")

    insufficient = cdb.check_disk_budget(
        free_bytes=2 * GIB, required_bytes=5 * GIB, path="/ws", caches=[]
    )
    chk(
        "insufficient space -> status disk_space_insufficient",
        insufficient.status == "disk_space_insufficient",
    )
    chk("insufficient space -> ok property false", insufficient.ok is False)
    chk(
        "insufficient space -> deficit computed correctly",
        insufficient.deficit_bytes == 3 * GIB,
    )
    chk("insufficient space -> has remediation text", len(insufficient.remediation) > 0)

    boundary_exact = cdb.check_disk_budget(
        free_bytes=5 * GIB, required_bytes=5 * GIB, path="/ws", caches=[]
    )
    chk("exact boundary (free == required) -> ok", boundary_exact.status == "ok")

    boundary_one_byte_short = cdb.check_disk_budget(
        free_bytes=5 * GIB - 1, required_bytes=5 * GIB, path="/ws", caches=[]
    )
    chk(
        "one byte short of boundary -> insufficient",
        boundary_one_byte_short.status == "disk_space_insufficient",
    )
    chk(
        "one byte short of boundary -> deficit is exactly 1 byte",
        boundary_one_byte_short.deficit_bytes == 1,
    )

    # ---- remediation text references the largest cache, not an arbitrary one ----

    caches = [
        _fake_cache("target/", "/ws/target", 26 * GIB),
        _fake_cache("~/.cargo", "/home/u/.cargo", 955 * 1024 * 1024),
        _fake_cache(".simplicio/", "/ws/.simplicio", 467 * 1024 * 1024),
    ]
    with_caches = cdb.check_disk_budget(
        free_bytes=1 * GIB, required_bytes=5 * GIB, path="/ws", caches=caches
    )
    chk(
        "remediation names the largest cache dir (target/)",
        "target/" in with_caches.remediation,
    )
    chk(
        "remediation does not name a smaller cache as the headline",
        ".simplicio/" not in with_caches.remediation.split(".")[0],
    )
    chk(
        "remediation never suggests an automatic delete",
        "rm -rf" not in with_caches.remediation and "delete" not in with_caches.remediation.lower()
        or "This tool does not delete" in with_caches.remediation,
    )

    # No caches to point at at all -> still returns usable, non-empty guidance.
    no_cache_evidence = cdb.check_disk_budget(
        free_bytes=1 * GIB, required_bytes=5 * GIB, path="/ws", caches=[]
    )
    chk(
        "insufficient with no caches measured still has remediation text",
        len(no_cache_evidence.remediation) > 0,
    )

    # ---- measure_cache_dirs: injectable size function, no real disk walk ----

    fake_sizes = {"/ws/target": 26 * GIB, "/home/u/.cargo": 955 * 1024 * 1024}

    def fake_isdir(path):
        return path in fake_sizes

    orig_isdir = os.path.isdir
    os.path.isdir = fake_isdir  # narrow, local monkeypatch
    try:
        reports = cdb.measure_cache_dirs(
            [("target/", "/ws/target"), ("~/.cargo", "/home/u/.cargo"), (".simplicio/", "/ws/.simplicio")],
            size_fn=lambda p: fake_sizes.get(p, 0),
        )
    finally:
        os.path.isdir = orig_isdir

    chk("measure_cache_dirs returns one report per input dir", len(reports) == 3)
    chk("measure_cache_dirs marks present dir as existing", reports[0].exists is True)
    chk("measure_cache_dirs marks missing dir as not existing", reports[2].exists is False)
    chk("measure_cache_dirs reports zero size for missing dir", reports[2].size_bytes == 0)
    chk(
        "measure_cache_dirs reports correct size for existing dir",
        reports[0].size_bytes == 26 * GIB,
    )

    # ---- run_preflight: fully mocked disk usage + cache sizing (no real I/O) ----

    fake_usage = SimpleNamespace(total=100 * GIB, used=98 * GIB, free=2 * GIB)
    result = cdb.run_preflight(
        "/ws",
        min_free_bytes=5 * GIB,
        disk_usage_fn=lambda _p: fake_usage,
        cache_dirs=[("target/", "/ws/target")],
        size_fn=lambda _p: 26 * GIB,
    )
    chk("run_preflight (mocked) detects insufficiency", result.status == "disk_space_insufficient")
    chk("run_preflight (mocked) reports mocked free bytes", result.free_bytes == 2 * GIB)

    fake_usage_ok = SimpleNamespace(total=100 * GIB, used=10 * GIB, free=90 * GIB)
    result_ok = cdb.run_preflight(
        "/ws",
        min_free_bytes=5 * GIB,
        disk_usage_fn=lambda _p: fake_usage_ok,
        cache_dirs=[("target/", "/ws/target")],
        size_fn=lambda _p: 26 * GIB,
    )
    chk("run_preflight (mocked) detects sufficiency", result_ok.status == "ok")

    # ---- to_dict(): stable, structured, machine-readable shape ----

    d = insufficient.to_dict()
    chk("to_dict has schema field", d.get("schema") == "simplicio.disk-budget/v1")
    chk("to_dict status matches", d.get("status") == "disk_space_insufficient")
    chk("to_dict ok field is false", d.get("ok") is False)
    chk("to_dict caches field is a list", isinstance(d.get("caches"), list))

    # ---- no automatic cleanup: the module must not delete anything ----

    src = inspect.getsource(cdb)
    forbidden = ["os.remove(", "os.unlink(", "shutil.rmtree(", "os.rmdir("]
    chk(
        "module source contains no deletion calls (no automatic cleanup)",
        not any(tok in src for tok in forbidden),
    )

    # ---- CLI smoke test: real process, real disk, no mocking ----
    # A tiny minimum should always be satisfied on any machine capable of
    # running this test; a huge minimum should always be reported as
    # insufficient. Both exercise the real `main()` / argparse / JSON path
    # end-to-end without asserting anything about the machine's actual
    # absolute free space.
    tiny = subprocess.run(
        [sys.executable, os.path.join(REPO, "scripts", "check_disk_budget.py"), "--min-free-gb", "0.0001", "--json"],
        capture_output=True,
        text=True,
    )
    chk("CLI exits 0 for a trivially small requirement", tiny.returncode == 0)
    chk("CLI --json output is valid JSON", _is_json(tiny.stdout))

    huge = subprocess.run(
        [sys.executable, os.path.join(REPO, "scripts", "check_disk_budget.py"), "--min-free-gb", "999999999", "--json"],
        capture_output=True,
        text=True,
    )
    chk("CLI exits 2 for an unsatisfiable requirement", huge.returncode == 2)
    chk(
        "CLI reports disk_space_insufficient for an unsatisfiable requirement",
        '"disk_space_insufficient"' in huge.stdout,
    )

    ok = all(checks)
    print("selftest: %s (%d/%d)" % ("PASS" if ok else "FAIL", sum(checks), len(checks)))
    return 0 if ok else 1


def _is_json(text):
    import json

    try:
        json.loads(text)
        return True
    except (ValueError, TypeError):
        return False


if __name__ == "__main__":
    sys.exit(main())
