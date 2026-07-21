from pathlib import Path
import tempfile

from scripts.release.generate_component_client import render


ROOT = Path(__file__).parents[2]
SCHEMA = ROOT / "docs/contracts/component-release-v1.schema.json"
GENERATED = ROOT / "crates/codegen/simplicio-runtime-client/src/generated.rs"


def test_generated_client_is_reproducible():
    expected = render(SCHEMA)
    assert GENERATED.read_text(encoding="utf-8") == expected

    with tempfile.TemporaryDirectory() as directory:
        regenerated = Path(directory) / "generated.rs"
        regenerated.write_text(expected, encoding="utf-8")
        assert regenerated.read_bytes() == GENERATED.read_bytes()
