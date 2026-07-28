from __future__ import annotations
import runpy
import sys
from pathlib import Path

def _script(name: str) -> Path:
    here = Path(__file__).resolve()
    for path in (here.parents[2] / "scripts" / name, here.parents[1] / "scripts" / name, Path.cwd() / "scripts" / name):
        if path.is_file():
            return path
    raise SystemExit(f"script not found: {name}")

def main() -> None:
    script = _script("check_json_boundaries.py")
    sys.argv[0] = str(script)
    runpy.run_path(str(script), run_name="__main__")
